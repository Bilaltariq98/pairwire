//! Session orchestration — wires all subsystems together for host and join modes.
//!
//! `run_host`: bind transport, start capture, accept connection, handshake, stream frames
//! `run_join`: connect to host, handshake, receive frames, decode, forward to viewer
//!
//! Encode/decode require the `encode` feature. Without it, the pipeline stubs out
//! gracefully with a warning after the handshake completes.
//!
//! Media transport uses a single moq-lite session with hang framing. Both screen
//! and audio are published as tracks in one broadcast, described by a hang catalog.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use pairwire_audio::{
    AudioCaptureThread, AudioDecodeThread, AudioEncodeThread, AudioPlaybackThread,
};
#[cfg(feature = "encode")]
use pairwire_capture::CaptureThread;
use pairwire_capture::DecodedFrame;
use pairwire_core::{
    SessionId,
    config::{AudioConfig, CaptureConfig, VideoEncoder},
};
use pairwire_input::{ClipboardReceiver, ClipboardSender, InputReceiver, InputSender};
use pairwire_protocol::{Capability, CursorUpdate, VideoCodec};
#[cfg(feature = "encode")]
use pairwire_transport::VideoStreamConfig;
use pairwire_transport::{
    AudioStreamConfig, ControlChannel, CursorReceiver, CursorSender, DatagramChannels,
    DrawingReceiver, DrawingSender, MoqSessionSubscriber, NegotiatedSession, QualityMonitor,
    StrokeCache, TAG_GUEST_AUDIO, Transport, datagram_router, guest_handshake, host_handshake,
    local_handshake, replay_persistent_strokes,
};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::cli::{HostArgs, JoinArgs};
use crate::guest_ctrl::guest_ctrl_state_task_fn;
use crate::host_ctrl::host_ctrl_state_task_fn;

type GuestAudioSenderParts = (
    Option<AudioCaptureThread>,
    Option<AudioEncodeThread>,
    Option<tokio::task::JoinHandle<()>>,
);

/// Maximum number of concurrent guests in the multi-guest spike.
/// Beyond this, new connections are dropped with a debug log.
const MAX_GUESTS: usize = 4;

/// The current state of the host session, published via a watch channel so
/// the overlay can display a status banner.
///
/// Lives in `app` rather than `core` because it is only meaningful in the
/// context of the host overlay UI.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostSessionState {
    /// No guest is connected; the host is waiting for an inbound connection.
    Waiting,
    /// One or more guests are actively connected.
    ///
    /// spike/multi-guest-p2p: extended from a single `peer_id_short: String`
    /// to a list of short IDs so the overlay can show "N guests: abc, def".
    Connected {
        /// First 8 hex characters of each connected guest's `EndpointId`.
        guests: Vec<String>,
    },
}

/// Channels sent from `run_host` to the host overlay window.
///
/// Created after the annotation stream and capture pipeline are set up and
/// forwarded to the main thread so `run_host_overlay` can launch the GUI.
///
/// Only available when the `gui` feature is compiled in.
#[cfg(feature = "gui")]
pub struct HostOverlayChannels {
    /// Accumulated strokes from `DrawingReceiver` (both peers).
    pub strokes_rx: watch::Receiver<Vec<pairwire_transport::TimedStroke>>,
    /// Live in-progress `DrawPoint`s from the remote peer.
    pub live_point_rx: watch::Receiver<Option<pairwire_protocol::drawing::DrawPoint>>,
    /// The overlay sends drawing events here; the session bridges them into
    /// the `DrawingSender` mpsc and out to the guest.
    pub drawing_event_tx: tokio::sync::broadcast::Sender<pairwire_transport::DrawingEvent>,
    /// Encoded capture resolution. Strokes from the guest are in this coordinate
    /// space, so the overlay needs it to map host-coord → screen-pixel correctly.
    pub capture_width: u32,
    pub capture_height: u32,
    /// Current host session state — updated before each `accept()` (Waiting)
    /// and after a successful handshake (Connected). When the sender is dropped
    /// (run_host returning), the overlay treats it as a shutdown signal and
    /// closes its window.
    pub state_rx: watch::Receiver<HostSessionState>,
}

/// Run as host: capture screen, accept connections in a loop, handshake, stream frames.
///
/// The host runs an accept loop so a guest that disconnects can reconnect to the
/// same host without the host needing to re-launch. Each iteration covers one
/// guest connection from `transport.accept()` through teardown. Long-lived state
/// (stroke cache, capture pipeline, overlay channels) is created once before the
/// loop and survives across reconnects.
///
/// `invite_tx` receives the invite URL as soon as it is generated, before the
/// first blocking `accept()` call. Pass `None` if you don't need the URL out-of-band.
///
/// `tray_action_rx` receives `ControlAction` events from the tray menu (Grant /
/// Revoke). Pass `None` when running without a tray (tests, non-tray builds).
///
/// `overlay_tx` receives the channels the host overlay window needs, once the
/// capture pipeline is established. Only sent on the first iteration. Pass `None`
/// to run headlessly (e.g. `--no-overlay` or the `gui` feature is not compiled).
///
/// `shutdown_rx` is an optional watch channel that becomes `true` when the
/// caller wants a graceful teardown (e.g. the host overlay window was closed).
/// When triggered, the host breaks out of its blocking `accept` / `conn.closed`
/// wait and runs its normal teardown sequence — including `transport.close()` —
/// before returning.  Pass `None` for headless / tray-only builds where the
/// session owns the process lifetime.
pub async fn run_host(
    args: HostArgs,
    invite_tx: Option<tokio::sync::oneshot::Sender<String>>,
    tray_action_rx: Option<tokio::sync::mpsc::Receiver<crate::control_state::ControlAction>>,
    #[cfg(feature = "gui")] overlay_tx: Option<tokio::sync::oneshot::Sender<HostOverlayChannels>>,
    #[cfg(not(feature = "gui"))] _overlay_tx: Option<()>,
    mut shutdown_rx: Option<watch::Receiver<bool>>,
) -> Result<()> {
    let transport = Transport::bind()
        .await
        .context("failed to bind iroh endpoint")?;

    let session_token: [u8; 16] = rand::random();
    let session_id = SessionId(session_token);
    let invite = transport
        .invite_link(session_id)
        .await
        .context("failed to generate invite link")?;

    println!("\n  Invite link:\n\n    {invite}\n");
    println!("  Waiting for guest to connect…\n");

    if let Some(tx) = invite_tx {
        let _ = tx.send(invite.to_string());
    }

    // Detect the physical display resolution so the encode pipeline doesn't
    // apply an unnecessary scale on Retina displays.
    let physical = pairwire_input::main_display_physical_size();
    let (width, height) = args.resolution.resolve(physical);
    info!(
        physical_w = physical.0,
        physical_h = physical.1,
        encode_w = width,
        encode_h = height,
        preset = ?args.resolution,
        "display resolution detected"
    );

    let capture_config = CaptureConfig {
        fps: args.fps,
        width,
        height,
        preferred_encoder: VideoEncoder::H264Software,
        ..Default::default()
    };
    capture_config
        .validate()
        .context("invalid capture config")?;
    AudioConfig::default()
        .validate()
        .context("invalid audio config")?;

    // ── Long-lived state (survives across reconnects) ──────────────────────────

    // Stroke cache: persists across guest disconnects so strokes are replayed to
    // a reconnecting guest. Declared before the loop so it lives for the entire
    // host session. See LEARNINGS.md 2026-05-04 for details.
    let stroke_cache = Arc::new(StrokeCache::new());

    // Session-state watch: overlay subscribes to this to show the status banner.
    // Dropping `state_tx` (when run_host returns) signals the overlay to close.
    #[cfg(feature = "gui")]
    let (session_state_tx, session_state_rx) =
        watch::channel::<HostSessionState>(HostSessionState::Waiting);

    // Broadcast sender for the host overlay window to emit drawing events.
    // Bridged into the mpsc that feeds DrawingSender so overlay strokes reach the guest.
    // Created once so the overlay's subscription stays alive across reconnects.
    #[cfg(feature = "gui")]
    let (overlay_drawing_bcast_tx, _overlay_drawing_rx0) =
        tokio::sync::broadcast::channel::<pairwire_transport::DrawingEvent>(32);
    // Watch channels so the overlay can render accumulated strokes and live points.
    // Created outside the loop so the overlay's receivers keep working across reconnects.
    #[cfg(feature = "gui")]
    let (host_strokes_watch_tx, host_strokes_watch_rx) =
        watch::channel::<Vec<pairwire_transport::TimedStroke>>(Vec::new());
    #[cfg(feature = "gui")]
    let (host_live_pt_watch_tx, host_live_pt_watch_rx) =
        watch::channel::<Option<pairwire_protocol::drawing::DrawPoint>>(None);

    // Capture pipeline — started once and kept warm across reconnects.
    // The tee thread forwards raw frames to the encode pipeline (per-connection)
    // and to the preview watch (overlay). When no guest is connected, the encode
    // receiver is dropped and the tee just drops frames silently (channel full =
    // back-pressure pauses capture callback naturally).
    //
    // We use an `Option<std::sync::mpsc::SyncSender<RawFrame>>` that is swapped
    // in on each new connection and dropped on teardown. The tee thread reads
    // from the capture rx and forwards to `encode_tx_slot` if it is Some.
    #[cfg(feature = "encode")]
    let capture_show_cursor = pairwire_capture::ShowCursorFlag::new(true);

    // We use an Arc<Mutex<Option<SyncSender>>> so the tee thread can swap the
    // encode sender each iteration without restarting.
    #[cfg(feature = "encode")]
    let encode_tx_slot: Arc<
        std::sync::Mutex<Option<std::sync::mpsc::SyncSender<pairwire_capture::RawFrame>>>,
    > = Arc::new(std::sync::Mutex::new(None));

    // One-shot: capture + tee are started on the first loop iteration and reused.
    #[cfg(feature = "encode")]
    let mut capture_started = false;
    #[cfg(feature = "encode")]
    let mut _capture_thread_holder: Option<CaptureThread> = None;
    #[cfg(feature = "encode")]
    let mut _tee_thread_holder: Option<std::thread::JoinHandle<()>> = None;

    // One-shot flag for sending the overlay channels (only on first iteration).
    #[cfg(feature = "gui")]
    let mut overlay_tx = overlay_tx;

    // Tray action relay — broadcast so each loop iteration can subscribe independently.
    // Each connection creates a fresh per-conn (action_tx, action_rx) and bridges from
    // the broadcast rx into action_tx, which merges with the auto-revoke watcher.
    let (tray_relay_bcast_tx, _tray_bcast_rx0) =
        tokio::sync::broadcast::channel::<crate::control_state::ControlAction>(8);
    if let Some(mut tray_rx) = tray_action_rx {
        let relay_tx = tray_relay_bcast_tx.clone();
        tokio::spawn(async move {
            while let Some(action) = tray_rx.recv().await {
                // Ignore send error — no subscribers is fine (no guest connected yet).
                let _ = relay_tx.send(action);
            }
        });
    }

    // ── Multi-guest fan-out state (spike/multi-guest-p2p) ─────────────────────
    //
    // Video fan-out: the encoder produces one stream of EncodedPackets. A pump
    // task reads them and broadcasts to all per-guest subscriber tasks. Each
    // per-guest task runs its own MoqSessionPublisher over its QUIC connection.
    //
    // Capacity 32: ~0.5s of keyframe interval at 60fps; lagging subscribers drop
    // frames (acceptable for a spike — see RecvError::Lagged handling below).
    #[cfg(feature = "encode")]
    let (video_fan_tx, _video_fan_rx0) =
        tokio::sync::broadcast::channel::<Arc<pairwire_capture::EncodedPacket>>(32);

    // Tracks which guest currently holds input control. Only one guest at a time
    // may send input events. Other guests are view-only. No UI for switching yet —
    // see PR description for followup work.
    //
    // TODO(multi-guest-control): add a tray item / hotkey to switch controlling guest.
    let controlling_guest: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // Active guest list used to update the overlay HostSessionState banner.
    let active_guests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // ── Accept loop ────────────────────────────────────────────────────────────

    loop {
        // Publish Waiting state before every accept so the overlay banner is
        // correct even after a guest disconnects and the loop iterates.
        #[cfg(feature = "gui")]
        let _ = session_state_tx.send(HostSessionState::Waiting);

        info!("waiting for guest to connect…");

        // Wait for an incoming guest connection, or bail early if shutdown fires
        // (e.g. host closes the overlay window before any guest connects).
        let accepted = if let Some(ref mut srx) = shutdown_rx {
            let shutdown_fut = async {
                loop {
                    if srx.changed().await.is_err() {
                        return;
                    }
                    if *srx.borrow() {
                        return;
                    }
                }
            };
            tokio::select! {
                result = transport.accept() => result.context("accept failed")?,
                () = shutdown_fut => {
                    info!("shutdown requested before guest connected — closing host");
                    transport.close().await;
                    return Ok(());
                }
            }
        } else {
            transport.accept().await.context("accept failed")?
        };
        let conn =
            Arc::new(accepted.ok_or_else(|| anyhow::anyhow!("endpoint closed before connection"))?);

        let peer_id_short = conn.remote_id().to_string()[..8].to_owned();
        info!(peer = %conn.remote_id(), "guest connected");

        // Track active guests and update the overlay banner.
        {
            let mut guests = active_guests.lock().unwrap_or_else(|e| e.into_inner());
            guests.push(peer_id_short.clone());
            #[cfg(feature = "gui")]
            let _ = session_state_tx.send(HostSessionState::Connected {
                guests: guests.clone(),
            });
        }

        // This first connected guest becomes the controlling guest (input forwarding).
        {
            let mut ctrl = controlling_guest.lock().unwrap_or_else(|e| e.into_inner());
            if ctrl.is_none() {
                *ctrl = Some(peer_id_short.clone());
                info!(peer = %peer_id_short, "guest is now the controlling guest");
            }
        }

        // ── Per-connection setup ───────────────────────────────────────────────

        // Perform handshake before starting media.
        let session = match perform_host_handshake(&conn, &capture_config, session_token).await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, peer = %conn.remote_id(), "handshake failed — looping");
                continue;
            }
        };
        info!(
            capabilities = ?session.capabilities,
            video_codec = ?session.video_codec,
            "handshake complete"
        );

        // Persistent control channel — must be the first post-handshake bidi accept.
        let (host_ctrl_outbound_tx, host_ctrl_outbound_rx) =
            tokio::sync::mpsc::channel::<pairwire_protocol::ControlMessage>(32);
        let ctrl_bi = conn.accept_bi().await;
        let (ctrl_send, ctrl_recv) = match ctrl_bi {
            Ok(Some(pair)) => pair,
            Ok(None) => {
                warn!("connection closed before control channel — looping");
                continue;
            }
            Err(e) => {
                warn!(error = %e, "failed to accept control channel — looping");
                continue;
            }
        };
        let host_ctrl_handle = ControlChannel::spawn(ctrl_send, ctrl_recv, host_ctrl_outbound_rx);
        info!("control channel accepted");

        // Signal the guest that the host is ready for follow-up bidi streams.
        if let Err(e) = host_ctrl_outbound_tx
            .send(pairwire_protocol::ControlMessage::HostReady)
            .await
        {
            warn!(error = %e, "failed to send HostReady — looping");
            host_ctrl_handle.stop().await;
            continue;
        }
        info!("host ready ack sent");

        // Probe stamped by the input receiver after each successful injection.
        let host_inject_probe = Arc::new(pairwire_input::InjectProbe::default());

        // Accept input bidi stream.
        let mut clipboard_recv_handle = None;
        let input_recv_handle = if session.has_capability(&Capability::RemoteControl) {
            match conn.accept_bi().await {
                Ok(Some((_input_send, input_recv))) => {
                    let (clipboard_sync_tx, clipboard_sync_rx) = tokio::sync::mpsc::channel(8);
                    let cb_receiver = ClipboardReceiver::new(clipboard_sync_rx);
                    clipboard_recv_handle = Some(cb_receiver.spawn());
                    let receiver = InputReceiver::new(input_recv)
                        .with_clipboard(clipboard_sync_tx)
                        .with_inject_probe(Arc::clone(&host_inject_probe));
                    let handle = receiver.spawn();
                    info!("input receiver started — permission: Denied (state machine will grant)");
                    Some(handle)
                }
                Ok(None) => {
                    warn!("connection closed before input stream");
                    None
                }
                Err(e) => {
                    warn!(error = %e, "failed to accept input stream");
                    None
                }
            }
        } else {
            None
        };

        // Accept annotation bidi stream.
        let mut host_drawing_send_handle = None;
        let mut host_drawing_recv_handle = None;
        let mut _host_draw_event_tx: Option<
            tokio::sync::mpsc::Sender<pairwire_transport::DrawingEvent>,
        > = None;
        let mut host_draw_point_tx = None;

        if session.has_capability(&Capability::Annotations) {
            // On a new connection, clear any strokes left from the previous guest
            // so the overlay starts fresh. The cache replay below will re-populate it.
            #[cfg(feature = "gui")]
            let _ = host_strokes_watch_tx.send(Vec::new());

            match conn.accept_bi().await {
                Ok(Some((annot_send, annot_recv))) => {
                    let (draw_event_tx, draw_event_rx) = tokio::sync::mpsc::channel(32);
                    let (draw_pt_tx, draw_pt_rx) = tokio::sync::mpsc::channel(64);
                    let sender = DrawingSender::new(Arc::clone(&conn), annot_send);
                    let (recv_handle, strokes_rx, live_rx) = DrawingReceiver::new(annot_recv)
                        .spawn_with_cache_and_echo(
                            draw_pt_rx,
                            Arc::clone(&stroke_cache),
                            draw_event_tx.clone(),
                        );

                    // Forward per-connection watches into the long-lived overlay watches.
                    // A fresh fwd task is spawned each connection; the previous one exits
                    // naturally when its per-conn rx is dropped on teardown.
                    #[cfg(feature = "gui")]
                    {
                        let fwd_strokes_tx = host_strokes_watch_tx.clone();
                        tokio::spawn(async move {
                            let mut rx = strokes_rx;
                            loop {
                                if rx.changed().await.is_err() {
                                    break;
                                }
                                let _ = fwd_strokes_tx.send(rx.borrow().clone());
                            }
                        });
                        let fwd_live_tx = host_live_pt_watch_tx.clone();
                        tokio::spawn(async move {
                            let mut rx = live_rx;
                            loop {
                                if rx.changed().await.is_err() {
                                    break;
                                }
                                let _ = fwd_live_tx.send(*rx.borrow());
                            }
                        });
                        // Bridge overlay broadcast → mpsc so host strokes reach DrawingSender.
                        let mpsc_tx_for_overlay = draw_event_tx.clone();
                        let mut overlay_bcast_rx = overlay_drawing_bcast_tx.subscribe();
                        tokio::spawn(async move {
                            loop {
                                match overlay_bcast_rx.recv().await {
                                    Ok(event) => {
                                        if mpsc_tx_for_overlay.send(event).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                        continue;
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                                }
                            }
                        });
                    }
                    #[cfg(not(feature = "gui"))]
                    let _ = (strokes_rx, live_rx);

                    replay_persistent_strokes(&stroke_cache, &draw_event_tx).await;
                    host_drawing_send_handle = Some(sender.spawn(draw_event_rx));
                    host_drawing_recv_handle = Some(recv_handle);
                    _host_draw_event_tx = Some(draw_event_tx);
                    host_draw_point_tx = Some(draw_pt_tx);
                    info!(
                        "annotation stream accepted; persistent stroke cache has {} strokes",
                        stroke_cache.len()
                    );
                }
                Ok(None) => warn!("connection closed before annotation stream"),
                Err(e) => warn!(error = %e, "failed to accept annotation stream"),
            }
        }

        let (cursor_chan_tx, cursor_chan_rx) = tokio::sync::mpsc::channel(64);
        let (guest_audio_chan_tx, guest_audio_chan_rx) = tokio::sync::mpsc::channel(64);
        let router = datagram_router::start(
            conn.raw_conn().clone(),
            DatagramChannels {
                cursor_tx: cursor_chan_tx,
                guest_audio_tx: Some(guest_audio_chan_tx),
                draw_point_tx: host_draw_point_tx,
                reaction_tx: None,
            },
        );

        let cursor_send = CursorSender::new(Arc::clone(&conn));
        let (cursor_update_tx, cursor_update_rx) = tokio::sync::mpsc::channel(1);
        let cursor_send_handle = cursor_send.spawn(cursor_update_rx);
        let cursor_frame_width = u16::try_from(capture_config.width).unwrap_or(u16::MAX);
        let cursor_frame_height = u16::try_from(capture_config.height).unwrap_or(u16::MAX);
        let cursor_poll_task =
            run_cursor_poll(cursor_update_tx, 0, cursor_frame_width, cursor_frame_height);

        let (host_cursor_recv_handle, mut host_cursor_watch) =
            CursorReceiver::spawn(cursor_chan_rx);
        let host_cursor_log_task = tokio::spawn(async move {
            while host_cursor_watch.changed().await.is_ok() {
                if let Some(update) = host_cursor_watch.borrow().clone() {
                    tracing::debug!(
                        peer_id = update.peer_id,
                        x = update.x,
                        y = update.y,
                        "guest cursor"
                    );
                }
            }
        });

        // Audio capture + encode pipeline (feeds into unified moq session).
        let (audio_capture_handle, audio_encode_handle, _audio_pkt_rx, _audio_stream_config) =
            match start_host_audio(&session) {
                Ok(parts) => parts,
                Err(e) => {
                    warn!(error = %e, "audio pipeline failed — continuing without audio");
                    (None, None, None, None)
                }
            };
        #[cfg(feature = "encode")]
        let (audio_pkt_rx, audio_stream_config) = (_audio_pkt_rx, _audio_stream_config);

        let mut guest_audio_decode_handle = None;
        let mut guest_audio_playback_handle = None;
        let guest_audio_forward_task = if session.has_capability(&Capability::Audio) {
            let audio_config = AudioConfig::default();
            let (guest_audio_decode_tx, guest_audio_decode_rx) = tokio::sync::mpsc::channel(64);
            match AudioDecodeThread::start(audio_config.clone(), guest_audio_decode_rx)
                .context("failed to start guest audio decoder")
            {
                Ok((guest_audio_decode, guest_audio_frame_rx)) => {
                    match AudioPlaybackThread::start(audio_config, guest_audio_frame_rx)
                        .context("failed to start guest audio playback")
                    {
                        Ok(guest_audio_playback) => {
                            guest_audio_decode_handle = Some(guest_audio_decode);
                            guest_audio_playback_handle = Some(guest_audio_playback);
                            let task = tokio::spawn(async move {
                                const FRAME_DURATION_US: u64 = 20_000;
                                let mut guest_audio_chan_rx = guest_audio_chan_rx;
                                let mut timestamp_us: u64 = 0;
                                while let Some(data) = guest_audio_chan_rx.recv().await {
                                    let packet = pairwire_audio::OpusPacket {
                                        data,
                                        timestamp_us,
                                        duration_ms: 20,
                                    };
                                    timestamp_us = timestamp_us.wrapping_add(FRAME_DURATION_US);
                                    if guest_audio_decode_tx.send(packet).await.is_err() {
                                        break;
                                    }
                                }
                            });
                            info!("guest audio pipeline started (host)");
                            Some(task)
                        }
                        Err(e) => {
                            warn!(error = %e, "guest audio playback failed");
                            Some(tokio::spawn(async move {
                                let mut guest_audio_chan_rx = guest_audio_chan_rx;
                                while guest_audio_chan_rx.recv().await.is_some() {}
                            }))
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "guest audio decoder failed");
                    Some(tokio::spawn(async move {
                        let mut guest_audio_chan_rx = guest_audio_chan_rx;
                        while guest_audio_chan_rx.recv().await.is_some() {}
                    }))
                }
            }
        } else {
            warn!("no Audio capability — not receiving guest audio");
            Some(tokio::spawn(async move {
                let mut guest_audio_chan_rx = guest_audio_chan_rx;
                while guest_audio_chan_rx.recv().await.is_some() {}
            }))
        };

        // Quality monitor — polls connection health and emits adapted configs.
        let quality_monitor =
            QualityMonitor::new(Arc::new(conn.raw_conn().clone()), capture_config.clone());
        let (quality_handle, _quality_rx, _quality_tier_rx) = quality_monitor.spawn();
        #[cfg(feature = "encode")]
        let quality_rx = _quality_rx;

        // Host control-state machine.
        let host_perm_handle = input_recv_handle
            .as_ref()
            .map(pairwire_input::InputReceiverHandle::permission_handle);

        // Per-connection action channel: merges tray relay + auto-revoke watcher.
        let (host_action_tx, mut host_action_rx) =
            tokio::sync::mpsc::channel::<crate::control_state::ControlAction>(8);
        // Bridge tray broadcast → per-conn mpsc. Exits when this iteration's
        // host_action_tx is dropped (end of connection teardown).
        {
            let mut tray_bcast_rx = tray_relay_bcast_tx.subscribe();
            let per_conn_action_tx = host_action_tx.clone();
            tokio::spawn(async move {
                use tokio::sync::broadcast::error::RecvError;
                loop {
                    match tray_bcast_rx.recv().await {
                        Ok(action) => {
                            if per_conn_action_tx.send(action).await.is_err() {
                                break;
                            }
                        }
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    }
                }
            });
        }

        #[cfg(feature = "encode")]
        let capture_show_cursor_conn = capture_show_cursor.clone();
        #[cfg(not(feature = "encode"))]
        let capture_show_cursor_conn = pairwire_capture::ShowCursorFlag::new(true);

        // Auto-revoke watcher.
        if let Some(ref perm) = host_perm_handle {
            let perm = Arc::clone(perm);
            let probe = Arc::clone(&host_inject_probe);
            let revoke_tx = host_action_tx.clone();
            tokio::spawn(async move {
                use crate::control_state::ControlAction;
                use pairwire_input::InputPermission;
                const QUIET_MS: u64 = 250;
                let mut last_pos = pairwire_input::current_cursor_position().ok();
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    interval.tick().await;
                    let cur = match pairwire_input::current_cursor_position() {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    let guest_has_control = perm.load_permission() != InputPermission::Denied;
                    if !guest_has_control {
                        last_pos = Some(cur);
                        continue;
                    }
                    let moved = last_pos.is_some_and(|prev| prev != cur);
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
                    let quiet = now_ms.saturating_sub(probe.last_ms()) > QUIET_MS;
                    if moved && quiet {
                        tracing::info!("host cursor moved during guest control — auto-revoking");
                        if revoke_tx.send(ControlAction::Revoke).await.is_err() {
                            break;
                        }
                    }
                    last_pos = Some(cur);
                }
            });
        }

        let auto_grant = args.allow_remote_control;
        let host_ctrl_outbound_for_task = host_ctrl_outbound_tx.clone();
        let mut host_ctrl_inbound_for_task = host_ctrl_handle.subscribe();
        let host_capture_show_cursor = capture_show_cursor_conn.clone();

        let host_ctrl_state_task = tokio::spawn(async move {
            host_ctrl_state_task_fn(
                auto_grant,
                &mut host_ctrl_inbound_for_task,
                &mut host_action_rx,
                &host_ctrl_outbound_for_task,
                &host_perm_handle,
                host_capture_show_cursor,
            )
            .await;
        });

        // ── Encode + fan-out publish (spike/multi-guest-p2p) ──────────────────
        //
        // Architecture:
        //   - Encoder produces EncodedPackets → fan-out pump task → broadcast channel
        //   - Each guest (first + additional) has a per-guest task that subscribes
        //     to the broadcast and drives its own MoqSessionPublisher.
        //   - Audio: fan-out is TODO — only the first guest receives host audio.
        //     See PR description for the design note.
        //   - Control: only the `controlling_guest` may send input. Others view-only.
        //
        // TODO(multi-guest-audio): broadcast audio packets the same way as video.
        // At 5 Mbps video + ~64 kbps audio per guest, host upstream = N × ~5.1 Mbps.
        // Residential upstreams (~25 Mbps) support N≤4. See PR description.
        #[cfg(feature = "encode")]
        {
            use pairwire_capture::EncodeThread;

            if session.video_codec.is_none() {
                warn!("no common video codec -- not sending frames");
            } else {
                // Start the capture pipeline on the first iteration; reuse on subsequent.
                if !capture_started {
                    let (capture_thread, raw_frame_rx) =
                        start_capture(capture_config.clone(), capture_show_cursor.clone())?;
                    _capture_thread_holder = Some(capture_thread);

                    // Send overlay channels now that capture is up (first iteration only).
                    #[cfg(feature = "gui")]
                    {
                        if let Some(tx) = overlay_tx.take() {
                            let channels = HostOverlayChannels {
                                strokes_rx: host_strokes_watch_rx.clone(),
                                live_point_rx: host_live_pt_watch_rx.clone(),
                                drawing_event_tx: overlay_drawing_bcast_tx.clone(),
                                capture_width: capture_config.width,
                                capture_height: capture_config.height,
                                state_rx: session_state_rx.clone(),
                            };
                            if tx.send(channels).is_err() {
                                warn!("host overlay receiver dropped before channels were sent");
                            } else {
                                info!("host overlay channels sent to main thread");
                            }
                        }
                    }

                    let encode_tx_slot_tee = Arc::clone(&encode_tx_slot);
                    let tee_thread = std::thread::spawn(move || {
                        while let Ok(raw) = raw_frame_rx.recv() {
                            // Forward to encode pipeline if connected; otherwise
                            // drop silently to keep capture warm.
                            let slot = encode_tx_slot_tee.lock().ok();
                            if let Some(guard) = slot {
                                if let Some(ref tx) = *guard {
                                    let _ = tx.try_send(raw);
                                }
                            }
                        }
                    });
                    _tee_thread_holder = Some(tee_thread);
                    capture_started = true;
                }

                // Create a fresh encode channel for this connection.
                let (tee_to_encode_tx, tee_to_encode_rx) =
                    std::sync::mpsc::sync_channel::<pairwire_capture::RawFrame>(4);
                // Install the new sender into the slot so the tee forwards frames.
                if let Ok(mut guard) = encode_tx_slot.lock() {
                    *guard = Some(tee_to_encode_tx);
                }

                let (encode_thread, mut video_pkt_rx) = EncodeThread::start_with_quality(
                    capture_config.clone(),
                    tee_to_encode_rx,
                    Some(quality_rx),
                )
                .context("failed to start encoder")?;

                let video_stream_config = VideoStreamConfig {
                    width: capture_config.width,
                    height: capture_config.height,
                    fps: f64::from(capture_config.fps),
                    bitrate: 5_000_000,
                };

                // ── Fan-out pump task ──────────────────────────────────────────
                // Reads encoded packets from the encoder and broadcasts them to
                // all per-guest subscriber tasks. Wrapping in Arc avoids cloning
                // the payload bytes (Bytes is an Arc internally, but the packet
                // struct itself needs to be cheap to clone into the broadcast).
                let fan_pump_fan_tx = video_fan_tx.clone();
                let fan_pump_task = tokio::spawn(async move {
                    while let Some(pkt) = video_pkt_rx.recv().await {
                        // If no subscribers yet, ignore the error.
                        let _ = fan_pump_fan_tx.send(Arc::new(pkt));
                    }
                });

                // ── Per-guest publisher for the first (primary) guest ──────────
                // Audio goes only to the primary guest (TODO(multi-guest-audio)).
                let first_guest_pub_handle = spawn_guest_publisher(
                    Arc::clone(&conn),
                    video_stream_config.clone(),
                    audio_stream_config,
                    video_fan_tx.subscribe(),
                    audio_pkt_rx,
                );

                // ── Multi-guest wait loop ──────────────────────────────────────
                // Accept additional connections and spawn per-guest publishers.
                // The primary guest's disconnect breaks us out of this loop.
                let mut shutdown_triggered = false;
                loop {
                    // Build the shutdown future from the optional receiver.
                    let shutdown_arm = async {
                        if let Some(ref mut srx) = shutdown_rx {
                            loop {
                                if srx.changed().await.is_err() {
                                    return;
                                }
                                if *srx.borrow() {
                                    return;
                                }
                            }
                        } else {
                            std::future::pending::<()>().await
                        }
                    };
                    tokio::select! {
                        _ = conn.closed() => {
                            info!(peer = %peer_id_short, "primary guest disconnected");
                            break;
                        }
                        () = shutdown_arm => {
                            info!("shutdown requested — closing host session");
                            shutdown_triggered = true;
                            break;
                        }
                        new_conn = transport.accept() => {
                            match new_conn {
                                Ok(Some(extra_conn)) => {
                                    let extra_peer = extra_conn.remote_id().to_string()[..8].to_owned();
                                    // Check guest cap.
                                    let current_count = {
                                        active_guests
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner())
                                            .len()
                                    };
                                    if current_count >= MAX_GUESTS {
                                        tracing::debug!(
                                            peer = %extra_peer,
                                            limit = MAX_GUESTS,
                                            "rejecting guest — MAX_GUESTS limit reached"
                                        );
                                        drop(extra_conn);
                                        continue;
                                    }

                                    info!(peer = %extra_peer, "additional guest connecting (fan-out)");

                                    // Add to active list and update state banner.
                                    {
                                        let mut guests = active_guests
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner());
                                        guests.push(extra_peer.clone());
                                        #[cfg(feature = "gui")]
                                        let _ = session_state_tx.send(
                                            HostSessionState::Connected {
                                                guests: guests.clone(),
                                            },
                                        );
                                    }

                                    // Spawn per-guest publisher for the additional guest.
                                    // Audio fan-out is TODO — extra guests get video only.
                                    let extra_conn = Arc::new(extra_conn);
                                    let extra_video_rx = video_fan_tx.subscribe();
                                    let extra_cfg = video_stream_config.clone();
                                    let extra_guests = Arc::clone(&active_guests);
                                    let extra_peer_clone = extra_peer.clone();
                                    #[cfg(feature = "gui")]
                                    let extra_state_tx = session_state_tx.clone();
                                    tokio::spawn(async move {
                                        // Lightweight handshake for the additional guest.
                                        // We reuse the same session_token so the guest can
                                        // verify session identity.
                                        //
                                        // TODO(multi-guest): full per-guest handshake,
                                        // including capability negotiation. For the spike
                                        // we skip this and go straight to moq publish.
                                        // This means additional guests must support H.264.
                                        let handle = spawn_guest_publisher(
                                            Arc::clone(&extra_conn),
                                            extra_cfg,
                                            None, // no audio for additional guests (TODO)
                                            extra_video_rx,
                                            None,
                                        );
                                        extra_conn.closed().await;
                                        info!(peer = %extra_peer_clone, "additional guest disconnected");
                                        handle.stop().await;
                                        // Remove from active list and update overlay.
                                        let mut guests = extra_guests
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner());
                                        guests.retain(|id| id != &extra_peer_clone);
                                        #[cfg(feature = "gui")]
                                        let _ = extra_state_tx.send(
                                            if guests.is_empty() {
                                                HostSessionState::Waiting
                                            } else {
                                                HostSessionState::Connected {
                                                    guests: guests.clone(),
                                                }
                                            },
                                        );
                                    });
                                }
                                Ok(None) => {
                                    // Endpoint closed — stop everything and exit.
                                    info!("transport endpoint closed");
                                    first_guest_pub_handle.stop().await;
                                    fan_pump_task.abort();
                                    let _ = fan_pump_task.await;
                                    encode_thread.stop();
                                    if let Ok(mut guard) = encode_tx_slot.lock() {
                                        *guard = None;
                                    }
                                    cursor_poll_task.abort();
                                    let _ = cursor_poll_task.await;
                                    host_cursor_log_task.abort();
                                    let _ = host_cursor_log_task.await;
                                    host_cursor_recv_handle.stop().await;
                                    quality_handle.stop().await;
                                    host_ctrl_state_task.abort();
                                    let _ = host_ctrl_state_task.await;
                                    host_ctrl_handle.stop().await;
                                    router.stop().await;
                                    cursor_send_handle.stop().await;
                                    teardown_drawing(
                                        host_drawing_send_handle,
                                        host_drawing_recv_handle,
                                    )
                                    .await;
                                    teardown_clipboard(clipboard_recv_handle).await;
                                    teardown_input(input_recv_handle).await;
                                    teardown_audio(
                                        guest_audio_forward_task,
                                        guest_audio_decode_handle,
                                        guest_audio_playback_handle,
                                        audio_encode_handle,
                                        audio_capture_handle,
                                    )
                                    .await;
                                    transport.close().await;
                                    return Ok(());
                                }
                                Err(e) => {
                                    warn!(error = %e, "transport accept error while connected");
                                }
                            }
                        }
                    }
                }
                if shutdown_triggered {
                    first_guest_pub_handle.stop().await;
                    fan_pump_task.abort();
                    let _ = fan_pump_task.await;
                    encode_thread.stop();
                    if let Ok(mut guard) = encode_tx_slot.lock() {
                        *guard = None;
                    }
                    cursor_poll_task.abort();
                    let _ = cursor_poll_task.await;
                    host_cursor_log_task.abort();
                    let _ = host_cursor_log_task.await;
                    host_cursor_recv_handle.stop().await;
                    quality_handle.stop().await;
                    host_ctrl_state_task.abort();
                    let _ = host_ctrl_state_task.await;
                    host_ctrl_handle.stop().await;
                    router.stop().await;
                    cursor_send_handle.stop().await;
                    teardown_drawing(host_drawing_send_handle, host_drawing_recv_handle).await;
                    teardown_clipboard(clipboard_recv_handle).await;
                    teardown_input(input_recv_handle).await;
                    teardown_audio(
                        guest_audio_forward_task,
                        guest_audio_decode_handle,
                        guest_audio_playback_handle,
                        audio_encode_handle,
                        audio_capture_handle,
                    )
                    .await;
                    transport.close().await;
                    return Ok(());
                }

                first_guest_pub_handle.stop().await;
                fan_pump_task.abort();
                let _ = fan_pump_task.await;
                encode_thread.stop();
                // Remove the encode sender from the slot so the tee drops frames
                // while waiting for the next guest.
                if let Ok(mut guard) = encode_tx_slot.lock() {
                    *guard = None;
                }
            }
        }

        #[cfg(not(feature = "encode"))]
        {
            warn!("encode feature not enabled -- handshake done but not sending frames");
            #[cfg(feature = "gui")]
            if let Some(tx) = overlay_tx.take() {
                let channels = HostOverlayChannels {
                    strokes_rx: host_strokes_watch_rx.clone(),
                    live_point_rx: host_live_pt_watch_rx.clone(),
                    drawing_event_tx: overlay_drawing_bcast_tx.clone(),
                    capture_width: capture_config.width,
                    capture_height: capture_config.height,
                    state_rx: session_state_rx.clone(),
                };
                let _ = tx.send(channels);
            }

            // Multi-guest wait loop (no-encode path): same fan-out accept loop
            // but without a publisher (just track state and reject over-limit).
            let mut shutdown_triggered_no_encode = false;
            loop {
                let shutdown_arm = async {
                    if let Some(ref mut srx) = shutdown_rx {
                        loop {
                            if srx.changed().await.is_err() {
                                return;
                            }
                            if *srx.borrow() {
                                return;
                            }
                        }
                    } else {
                        std::future::pending::<()>().await
                    }
                };
                tokio::select! {
                    _ = conn.closed() => {
                        info!(peer = %peer_id_short, "primary guest disconnected");
                        break;
                    }
                    () = shutdown_arm => {
                        info!("shutdown requested — closing host session");
                        shutdown_triggered_no_encode = true;
                        break;
                    }
                    new_conn = transport.accept() => {
                        match new_conn {
                            Ok(Some(extra_conn)) => {
                                let extra_peer = extra_conn.remote_id().to_string()[..8].to_owned();
                                let current_count = {
                                    active_guests
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .len()
                                };
                                if current_count >= MAX_GUESTS {
                                    tracing::debug!(
                                        peer = %extra_peer,
                                        limit = MAX_GUESTS,
                                        "rejecting guest — MAX_GUESTS limit reached (no-encode path)"
                                    );
                                    drop(extra_conn);
                                    continue;
                                }
                                info!(peer = %extra_peer, "additional guest connected (no-encode, view-only)");
                                {
                                    let mut guests = active_guests
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    guests.push(extra_peer.clone());
                                    #[cfg(feature = "gui")]
                                    let _ = session_state_tx.send(HostSessionState::Connected {
                                        guests: guests.clone(),
                                    });
                                }
                                let extra_guests = Arc::clone(&active_guests);
                                let extra_peer_clone = extra_peer.clone();
                                #[cfg(feature = "gui")]
                                let extra_state_tx = session_state_tx.clone();
                                let extra_conn = Arc::new(extra_conn);
                                tokio::spawn(async move {
                                    extra_conn.closed().await;
                                    info!(peer = %extra_peer_clone, "additional guest disconnected (no-encode)");
                                    let mut guests = extra_guests
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner());
                                    guests.retain(|id| id != &extra_peer_clone);
                                    #[cfg(feature = "gui")]
                                    let _ = extra_state_tx.send(
                                        if guests.is_empty() {
                                            HostSessionState::Waiting
                                        } else {
                                            HostSessionState::Connected {
                                                guests: guests.clone(),
                                            }
                                        },
                                    );
                                });
                            }
                            Ok(None) => {
                                info!("transport endpoint closed");
                                cursor_poll_task.abort();
                                let _ = cursor_poll_task.await;
                                host_cursor_log_task.abort();
                                let _ = host_cursor_log_task.await;
                                host_cursor_recv_handle.stop().await;
                                quality_handle.stop().await;
                                host_ctrl_state_task.abort();
                                let _ = host_ctrl_state_task.await;
                                host_ctrl_handle.stop().await;
                                router.stop().await;
                                cursor_send_handle.stop().await;
                                teardown_drawing(
                                    host_drawing_send_handle,
                                    host_drawing_recv_handle,
                                )
                                .await;
                                teardown_clipboard(clipboard_recv_handle).await;
                                teardown_input(input_recv_handle).await;
                                teardown_audio(
                                    guest_audio_forward_task,
                                    guest_audio_decode_handle,
                                    guest_audio_playback_handle,
                                    audio_encode_handle,
                                    audio_capture_handle,
                                )
                                .await;
                                transport.close().await;
                                return Ok(());
                            }
                            Err(e) => {
                                warn!(error = %e, "transport accept error while connected");
                            }
                        }
                    }
                }
            }
            if shutdown_triggered_no_encode {
                cursor_poll_task.abort();
                let _ = cursor_poll_task.await;
                host_cursor_log_task.abort();
                let _ = host_cursor_log_task.await;
                host_cursor_recv_handle.stop().await;
                quality_handle.stop().await;
                host_ctrl_state_task.abort();
                let _ = host_ctrl_state_task.await;
                host_ctrl_handle.stop().await;
                router.stop().await;
                cursor_send_handle.stop().await;
                teardown_drawing(host_drawing_send_handle, host_drawing_recv_handle).await;
                teardown_clipboard(clipboard_recv_handle).await;
                teardown_input(input_recv_handle).await;
                teardown_audio(
                    guest_audio_forward_task,
                    guest_audio_decode_handle,
                    guest_audio_playback_handle,
                    audio_encode_handle,
                    audio_capture_handle,
                )
                .await;
                transport.close().await;
                return Ok(());
            }
        }

        // ── Per-connection teardown (loop continues to next accept) ────────────
        cursor_poll_task.abort();
        let _ = cursor_poll_task.await;
        host_cursor_log_task.abort();
        let _ = host_cursor_log_task.await;
        host_cursor_recv_handle.stop().await;
        quality_handle.stop().await;
        host_ctrl_state_task.abort();
        let _ = host_ctrl_state_task.await;
        host_ctrl_handle.stop().await;
        router.stop().await;
        cursor_send_handle.stop().await;
        teardown_drawing(host_drawing_send_handle, host_drawing_recv_handle).await;
        teardown_clipboard(clipboard_recv_handle).await;
        teardown_input(input_recv_handle).await;
        teardown_audio(
            guest_audio_forward_task,
            guest_audio_decode_handle,
            guest_audio_playback_handle,
            audio_encode_handle,
            audio_capture_handle,
        )
        .await;

        // Remove this guest from the active list and update the overlay banner.
        {
            let mut guests = active_guests.lock().unwrap_or_else(|e| e.into_inner());
            guests.retain(|id| id != &peer_id_short);
            // If the disconnecting guest held control, clear the slot so the
            // next guest can claim it on connect (or a future UI can assign it).
            let mut ctrl = controlling_guest.lock().unwrap_or_else(|e| e.into_inner());
            if ctrl.as_deref() == Some(&peer_id_short) {
                *ctrl = None;
                info!(peer = %peer_id_short, "controlling guest disconnected — control released");
            }
        }

        info!("per-connection teardown complete; ready for next guest");
    }

    // Unreachable: the loop only exits via explicit `return Ok(())` above when the
    // transport endpoint closes (or via `?` propagation on fatal errors).
    #[allow(unreachable_code)]
    {
        transport.close().await;
        Ok(())
    }
}

// ── Multi-guest helpers (spike/multi-guest-p2p) ───────────────────────────────

/// Spawn a `MoqSessionPublisher` for a single guest connection that subscribes to
/// the video fan-out broadcast channel.
///
/// Each call creates a new tokio mpsc channel bridging the broadcast receiver into
/// the `MoqSessionPublisher`. This means every guest gets its own independent
/// QUIC stream set, even though they all receive the same encoded frames.
///
/// `audio_rx` is `Some` only for the primary guest; additional guests pass `None`
/// until `TODO(multi-guest-audio)` is implemented.
///
/// Keyframe-on-join: new subscribers that arrive mid-stream will see `Lagged`
/// errors until the next IDR frame arrives in the broadcast. The moq subscriber on
/// the guest side already handles the "wait for keyframe" invariant in its decode
/// loop (see `DecodeThread`). We do not force a keyframe here — the encoder emits
/// one on its regular interval (default: every `keyframe_interval_secs`).
///
/// TODO(multi-guest-keyframe): add `force_keyframe: Arc<AtomicBool>` to
/// `EncodeThread` so joining guests can request an immediate IDR without waiting
/// for the next scheduled keyframe.
#[cfg(feature = "encode")]
fn spawn_guest_publisher(
    conn: Arc<pairwire_transport::PairwireConnection>,
    video_config: pairwire_transport::VideoStreamConfig,
    audio_config: Option<pairwire_transport::AudioStreamConfig>,
    mut video_fan_rx: tokio::sync::broadcast::Receiver<Arc<pairwire_capture::EncodedPacket>>,
    audio_pkt_rx: Option<tokio::sync::mpsc::Receiver<pairwire_audio::OpusPacket>>,
) -> pairwire_transport::MoqSessionPublisherHandle {
    // Bridge: broadcast receiver → mpsc sender consumed by MoqSessionPublisher.
    let (video_bridge_tx, video_bridge_rx) =
        tokio::sync::mpsc::channel::<pairwire_capture::EncodedPacket>(32);

    // Pump task: reads from the broadcast and forwards into the per-guest mpsc.
    tokio::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match video_fan_rx.recv().await {
                Ok(pkt) => {
                    // Arc deref clone: cheap, just copies the EncodedPacket struct
                    // (Bytes fields are themselves Arc-backed, so this is O(1)).
                    if video_bridge_tx
                        .send(pairwire_capture::EncodedPacket::clone(&pkt))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                // Lagged: this guest fell behind. Skip the dropped frames; the guest
                // decoder will discard delta frames until the next keyframe arrives
                // (see DecodeThread's `waiting_for_keyframe` logic).
                Err(RecvError::Lagged(n)) => {
                    tracing::debug!(dropped = n, "video fan-out: guest subscriber lagged");
                    continue;
                }
                Err(RecvError::Closed) => break,
            }
        }
    });

    let publisher = pairwire_transport::MoqSessionPublisher::new(conn);
    publisher.spawn(video_config, audio_config, video_bridge_rx, audio_pkt_rx)
}

// ── Teardown helpers ──────────────────────────────────────────────────────────

async fn teardown_drawing(
    send_handle: Option<pairwire_transport::DrawingSenderHandle>,
    recv_handle: Option<pairwire_transport::DrawingReceiverHandle>,
) {
    if let Some(handle) = send_handle {
        handle.stop().await;
    }
    if let Some(handle) = recv_handle {
        handle.stop().await;
    }
}

async fn teardown_clipboard(recv_handle: Option<pairwire_input::ClipboardReceiverHandle>) {
    if let Some(handle) = recv_handle {
        handle.stop().await;
    }
}

async fn teardown_input(recv_handle: Option<pairwire_input::InputReceiverHandle>) {
    if let Some(handle) = recv_handle {
        handle.stop().await;
    }
}

async fn teardown_audio(
    forward_task: Option<tokio::task::JoinHandle<()>>,
    decode_handle: Option<AudioDecodeThread>,
    playback_handle: Option<AudioPlaybackThread>,
    encode_handle: Option<AudioEncodeThread>,
    capture_handle: Option<AudioCaptureThread>,
) {
    if let Some(task) = forward_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(decode) = decode_handle {
        decode.stop();
    }
    if let Some(playback) = playback_handle {
        playback.stop();
    }
    if let Some(encode) = encode_handle {
        encode.stop();
    }
    if let Some(capture) = capture_handle {
        capture.stop();
    }
}

/// Run as guest: connect to host, handshake, receive and decode frames, forward to viewer.
///
/// `viewer_input_rx` receives input events from the viewer UI (keyboard/mouse).
/// `drawing_event_rx` receives drawing events from the viewer (strokes, points, clears).
/// `drawing_tx` publishes the accumulated stroke list to the viewer whenever it changes.
/// `mute_rx` receives mute toggle commands from the viewer (`true` = muted).
/// `quality_tx` publishes the current connection quality tier to the viewer.
/// `control_state_tx` publishes the current `RuntimeControlState` to the viewer's HUD.
/// `control_request_rx` receives `ControlRequest` messages from the viewer and forwards them to
/// the host via the control channel.
///
/// `shutdown_rx` is a watch channel that becomes `true` when the caller wants
/// graceful teardown (e.g. the GUI window closed). When triggered, `run_join`
/// breaks out of its main `conn.closed()` wait and runs its normal teardown
/// sequence — including `transport.close().await` — before returning.
#[allow(clippy::too_many_arguments)]
pub async fn run_join(
    args: JoinArgs,
    frame_tx: watch::Sender<Option<DecodedFrame>>,
    cursor_tx: watch::Sender<Option<CursorUpdate>>,
    drawing_tx: watch::Sender<Vec<pairwire_transport::TimedStroke>>,
    live_point_tx: watch::Sender<Option<pairwire_protocol::drawing::DrawPoint>>,
    drawing_event_rx: tokio::sync::mpsc::Receiver<pairwire_transport::DrawingEvent>,
    mute_rx: watch::Receiver<bool>,
    quality_tx: watch::Sender<pairwire_transport::QualityTier>,
    mut viewer_input_rx: tokio::sync::mpsc::Receiver<pairwire_protocol::input::InputMessage>,
    control_state_tx: watch::Sender<crate::control_state::RuntimeControlState>,
    mut control_request_rx: tokio::sync::mpsc::Receiver<pairwire_protocol::ControlMessage>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    #[cfg(not(feature = "encode"))]
    let _ = &frame_tx;

    let invite: pairwire_transport::InviteLink =
        args.invite_url.parse().context("invalid invite link")?;

    let transport = Transport::bind()
        .await
        .context("failed to bind iroh endpoint")?;

    let conn = Arc::new(
        transport
            .connect(&invite)
            .await
            .context("failed to connect to host")?,
    );
    info!(peer = %conn.remote_id(), "connected to host");

    // Perform handshake before starting media.
    let session = perform_guest_handshake(&conn, invite.session_id.0).await?;
    info!(
        capabilities = ?session.capabilities,
        video_codec = ?session.video_codec,
        "handshake complete"
    );

    // Persistent control channel — must be the first post-handshake bidi open so
    // the ordinal position matches the host's `accept_bi` sequence above.
    // guest_ctrl_outbound_tx is used below to flush the stream and to forward
    // ControlRequest messages from the viewer.
    let (guest_ctrl_outbound_tx, guest_ctrl_outbound_rx) =
        tokio::sync::mpsc::channel::<pairwire_protocol::ControlMessage>(32);
    let (ctrl_send, ctrl_recv) = conn
        .open_control_stream()
        .await
        .context("failed to open control channel stream")?;
    let guest_ctrl_handle = ControlChannel::spawn(ctrl_send, ctrl_recv, guest_ctrl_outbound_rx);

    // Force the QUIC bidi stream open by sending an initial message — open_bi
    // alone doesn't announce the stream to the remote's accept_bi until the first
    // STREAM frame is written. SetSharing is used as the flush trigger so the host's
    // accept_bi can return and the host can proceed to send HostReady.
    guest_ctrl_outbound_tx
        .send(pairwire_protocol::ControlMessage::SetSharing {
            screen: true,
            audio: true,
        })
        .await
        .ok();
    info!("control channel opened");

    // Await the host's HostReady ack before opening follow-up bidi streams.
    // This replaces the former 250 ms timing guard with an explicit handshake.
    // A 5-second timeout preserves backward-compatibility with older host builds
    // that do not emit HostReady — we log a warning and proceed anyway.
    {
        use tokio::sync::broadcast::error::RecvError;
        let mut ready_rx = guest_ctrl_handle.subscribe();
        let ready = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match ready_rx.recv().await {
                    Ok(pairwire_protocol::ControlMessage::HostReady) => return Ok(()),
                    Ok(_) => continue, // ignore other messages while waiting
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => {
                        return Err(anyhow::anyhow!("control channel closed before HostReady"));
                    }
                }
            }
        })
        .await;
        match ready {
            Ok(Ok(())) => info!("received host ready ack"),
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                warn!("HostReady timed out after 5s — proceeding anyway (host may be old build)")
            }
        }
    }

    // Guest control-state machine: mirrors the host's ControlGrant / ControlRevoke
    // messages and updates a watch channel so the viewer can reflect the state.
    // `control_state_tx` is provided by the caller (main.rs) and shared with the viewer.
    let guest_ctrl_inbound_for_task = guest_ctrl_handle.subscribe();
    let guest_ctrl_state_task = tokio::spawn(guest_ctrl_state_task_fn(
        guest_ctrl_inbound_for_task,
        control_state_tx,
    ));

    // Forward ControlRequest messages from the viewer to the host via the control channel.
    let ctrl_outbound_tx_for_fwd = guest_ctrl_outbound_tx.clone();
    let control_request_fwd_task = tokio::spawn(async move {
        while let Some(msg) = control_request_rx.recv().await {
            if ctrl_outbound_tx_for_fwd.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Quality monitor on the guest side — drives the HUD quality dot.
    // Uses a dummy CaptureConfig since the guest doesn't encode; we only need the tier.
    let guest_quality_monitor =
        QualityMonitor::new(Arc::new(conn.raw_conn().clone()), CaptureConfig::default());
    let (guest_quality_handle, _guest_quality_config_rx, guest_quality_tier_rx) =
        guest_quality_monitor.spawn();

    // Forward tier changes to the viewer's quality watch channel.
    let quality_fwd_tx = quality_tx;
    tokio::spawn(async move {
        let mut rx = guest_quality_tier_rx;
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let _ = quality_fwd_tx.send(*rx.borrow());
        }
    });

    // Open input bidi stream (guest→host) before moq-lite starts.
    // Forward events from the viewer's input channel to the InputSender.
    // ClipboardSender shares the same channel so clipboard events ride the same stream.
    let mut guest_clipboard_send_handle = None;
    let (input_send_handle, input_forward_task) =
        if session.has_capability(&Capability::RemoteControl) {
            match conn.open_control_stream().await {
                Ok((input_send, _input_recv)) => {
                    let (tx, rx) =
                        tokio::sync::mpsc::channel::<pairwire_protocol::input::InputMessage>(32);
                    let sender = InputSender::new(input_send);
                    let handle = sender.spawn(rx);

                    // ClipboardSender monitors the guest clipboard and pushes
                    // InputMessage::Clipboard onto the same stream as keyboard/mouse.
                    let cb_sender = ClipboardSender::new(tx.clone(), true);
                    guest_clipboard_send_handle = Some(cb_sender.spawn());

                    // Forward viewer input events to the InputSender channel.
                    let forward = tokio::spawn(async move {
                        while let Some(msg) = viewer_input_rx.recv().await {
                            if tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                    });
                    info!("input sender started");
                    (Some(handle), Some(forward))
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "failed to open input stream — remote control disabled"
                    );
                    (None, None)
                }
            }
        } else {
            (None, None)
        };

    // Open annotation bidi stream (guest→host) before moq-lite starts.
    // Carries DrawStroke / DrawClear messages in both directions.
    // Must be declared before the datagram router so guest_draw_point_tx is in scope.
    let mut guest_drawing_send_handle = None;
    let mut guest_drawing_recv_handle = None;
    let mut guest_draw_point_tx = None;

    if session.has_capability(&Capability::Annotations) {
        match conn.open_control_stream().await {
            Ok((annot_send, annot_recv)) => {
                let (draw_pt_tx, draw_pt_rx) = tokio::sync::mpsc::channel(64);
                let sender = DrawingSender::new(Arc::clone(&conn), annot_send);
                let (recv_handle, strokes_rx, live_rx) =
                    DrawingReceiver::new(annot_recv).spawn(draw_pt_rx);

                // Forward received strokes to the viewer's drawing watch channel.
                let drawing_fwd_tx = drawing_tx.clone();
                tokio::spawn(async move {
                    let mut rx = strokes_rx;
                    loop {
                        if rx.changed().await.is_err() {
                            break;
                        }
                        let _ = drawing_fwd_tx.send(rx.borrow().clone());
                    }
                });

                // Forward live DrawPoint datagrams to the viewer for in-progress preview.
                let live_fwd_tx = live_point_tx.clone();
                tokio::spawn(async move {
                    let mut rx = live_rx;
                    loop {
                        if rx.changed().await.is_err() {
                            break;
                        }
                        let _ = live_fwd_tx.send(*rx.borrow());
                    }
                });

                // DrawingSender consumes drawing_event_rx directly — viewer events
                // flow straight to the sender without an extra bridge channel.
                guest_drawing_send_handle = Some(sender.spawn(drawing_event_rx));
                guest_drawing_recv_handle = Some(recv_handle);
                guest_draw_point_tx = Some(draw_pt_tx);
                info!("annotation stream opened");
            }
            Err(e) => warn!(error = %e, "failed to open annotation stream"),
        }
    }

    let (cursor_chan_tx, cursor_chan_rx) = tokio::sync::mpsc::channel(64);
    let router = datagram_router::start(
        conn.raw_conn().clone(),
        DatagramChannels {
            cursor_tx: cursor_chan_tx,
            guest_audio_tx: None,
            draw_point_tx: guest_draw_point_tx,
            reaction_tx: None,
        },
    );

    let (cursor_recv_handle, mut cursor_watch_rx) = CursorReceiver::spawn(cursor_chan_rx);
    let cursor_watch_tx = cursor_tx.clone();
    let cursor_forward_task = tokio::spawn(async move {
        loop {
            if cursor_watch_rx.changed().await.is_err() {
                break;
            }
            let _ = cursor_watch_tx.send(cursor_watch_rx.borrow().clone());
        }
    });

    // Guest cursor sender — polls the guest's own cursor and sends datagrams to the host.
    // peer_id=1 identifies the guest. Frame dims are 0,0 so cursor stays in guest display
    // coords (the host has no video frame from the guest to scale into).
    let guest_cursor_send = CursorSender::new(Arc::clone(&conn));
    let (guest_cursor_update_tx, guest_cursor_update_rx) = tokio::sync::mpsc::channel(1);
    let guest_cursor_send_handle = guest_cursor_send.spawn(guest_cursor_update_rx);
    let guest_cursor_poll_task = run_cursor_poll(guest_cursor_update_tx, 1, 0, 0);

    // Prepare video decode channel.
    let (pkt_tx, _pkt_rx) = std::sync::mpsc::sync_channel::<pairwire_capture::EncodedPacket>(8);
    #[cfg(feature = "encode")]
    let pkt_rx = _pkt_rx;

    // Audio decode + playback pipeline.
    let has_audio = session.has_capability(&Capability::Audio);
    let mut audio_pkt_tx = None;
    let mut audio_decode_handle = None;
    let mut audio_playback_handle = None;

    if has_audio {
        let audio_config = AudioConfig::default();
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        let (audio_decode, audio_frame_rx) = AudioDecodeThread::start(audio_config.clone(), rx)
            .context("failed to start audio decoder")?;
        let audio_playback = AudioPlaybackThread::start(audio_config, audio_frame_rx)
            .context("failed to start audio playback")?;
        audio_pkt_tx = Some(tx);
        audio_decode_handle = Some(audio_decode);
        audio_playback_handle = Some(audio_playback);
        info!("audio pipeline started (guest)");
    } else {
        warn!("no Audio capability — not receiving audio");
    }

    // Unified moq subscriber — receives both screen and audio tracks.
    let subscriber = MoqSessionSubscriber::new(Arc::clone(&conn));
    let sub_handle = subscriber.spawn(pkt_tx, audio_pkt_tx);

    let (guest_audio_capture_handle, guest_audio_encode_handle, guest_audio_send_task) =
        start_guest_audio_sender(&session, Arc::clone(&conn)).await?;

    // Watch for mute toggle commands from the viewer and apply them to the capture thread.
    // We clone the muted AtomicBool flag from the capture thread so the task can
    // toggle it without holding the capture handle.
    let mute_task = if let Some(ref capture) = guest_audio_capture_handle {
        let muted_flag = capture.muted_flag();
        let mut mute_watch = mute_rx;
        Some(tokio::spawn(async move {
            loop {
                if mute_watch.changed().await.is_err() {
                    break;
                }
                let should_mute = *mute_watch.borrow();
                muted_flag.store(should_mute, std::sync::atomic::Ordering::Relaxed);
                info!(muted = should_mute, "mic mute state changed");
            }
        }))
    } else {
        // No audio capture — consume mute_rx to avoid unused variable warning.
        drop(mute_rx);
        None
    };

    // Helper async block that resolves when `shutdown_rx` becomes true.
    // Uses `changed()` rather than `wait_for()` because `wait_for()` returns a
    // `RwLockReadGuard` which is `!Send` and therefore cannot be held across an
    // `.await` inside a `tokio::spawn`-ed (Send) future.
    let shutdown_fut = {
        let mut rx = shutdown_rx;
        async move {
            loop {
                if rx.changed().await.is_err() {
                    return; // sender dropped → treat as shutdown
                }
                if *rx.borrow() {
                    return;
                }
            }
        }
    };
    // Pin the future so it can be polled in `select!` across multiple branches.
    tokio::pin!(shutdown_fut);

    #[cfg(feature = "encode")]
    {
        use pairwire_capture::DecodeThread;

        if session.video_codec.is_none() {
            warn!("no common video codec — not decoding frames");
            tokio::select! {
                _ = conn.closed() => { info!("host disconnected"); }
                () = &mut shutdown_fut => { info!("shutdown requested — closing session"); }
            }
        } else {
            let (decode_thread, mut decoded_rx) =
                DecodeThread::start(pkt_rx).context("failed to start decoder")?;

            tokio::spawn(async move {
                while let Some(frame) = decoded_rx.recv().await {
                    let _ = frame_tx.send(Some(frame));
                }
            });

            tokio::select! {
                _ = conn.closed() => { info!("host disconnected"); }
                () = &mut shutdown_fut => { info!("shutdown requested — closing session"); }
            }
            decode_thread.stop();
        }
    }

    #[cfg(not(feature = "encode"))]
    {
        warn!("encode feature not enabled — handshake done but not decoding frames");
        tokio::select! {
            _ = conn.closed() => { info!("host disconnected"); }
            () = &mut shutdown_fut => { info!("shutdown requested — closing session"); }
        }
    }

    sub_handle.stop().await;
    guest_quality_handle.stop().await;
    guest_ctrl_state_task.abort();
    let _ = guest_ctrl_state_task.await;
    control_request_fwd_task.abort();
    let _ = control_request_fwd_task.await;
    guest_ctrl_handle.stop().await;
    router.stop().await;
    cursor_forward_task.abort();
    let _ = cursor_forward_task.await;
    cursor_recv_handle.stop().await;
    guest_cursor_poll_task.abort();
    let _ = guest_cursor_poll_task.await;
    guest_cursor_send_handle.stop().await;
    if let Some(handle) = guest_drawing_send_handle {
        handle.stop().await;
    }
    if let Some(handle) = guest_drawing_recv_handle {
        handle.stop().await;
    }
    if let Some(task) = mute_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(handle) = guest_clipboard_send_handle {
        handle.stop().await;
    }
    if let Some(task) = input_forward_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(handle) = input_send_handle {
        handle.stop().await;
    }
    if let Some(task) = guest_audio_send_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(encode) = guest_audio_encode_handle {
        encode.stop();
    }
    if let Some(capture) = guest_audio_capture_handle {
        capture.stop();
    }
    if let Some(decode) = audio_decode_handle {
        decode.stop();
    }
    if let Some(playback) = audio_playback_handle {
        playback.stop();
    }

    transport.close().await;
    Ok(())
}

/// Open the control stream and perform the host-side handshake.
async fn perform_host_handshake(
    conn: &pairwire_transport::PairwireConnection,
    config: &CaptureConfig,
    session_token: [u8; 16],
) -> Result<NegotiatedSession> {
    let (send, recv) = conn
        .open_control_stream()
        .await
        .context("failed to open control stream")?;

    #[allow(clippy::cast_possible_truncation)] // display dimensions are always < 65535
    let mut local = local_handshake(
        host_capabilities(),
        config.width as u16,
        config.height as u16,
        100, // scale factor — real value from display API in Phase 3
        session_token,
    );
    local.video_codecs = available_video_codecs();

    let (_send, _recv, session) = host_handshake(send, recv, local)
        .await
        .context("handshake failed")?;

    Ok(session)
}

/// Accept the control stream and perform the guest-side handshake.
async fn perform_guest_handshake(
    conn: &pairwire_transport::PairwireConnection,
    session_token: [u8; 16],
) -> Result<NegotiatedSession> {
    let (send, recv) = conn
        .accept_bi()
        .await
        .context("failed to accept control stream")?
        .ok_or_else(|| anyhow::anyhow!("connection closed before control stream"))?;

    let mut local = local_handshake(
        guest_capabilities(),
        0, // guest doesn't share a display
        0,
        0,
        session_token,
    );
    local.video_codecs = available_video_codecs();

    let (_send, _recv, session) = guest_handshake(send, recv, local)
        .await
        .context("handshake failed")?;

    Ok(session)
}

/// Build the host-side audio capture + encode pipeline if both peers support audio.
#[allow(clippy::type_complexity)]
fn start_host_audio(
    session: &NegotiatedSession,
) -> Result<(
    Option<AudioCaptureThread>,
    Option<AudioEncodeThread>,
    Option<tokio::sync::mpsc::Receiver<pairwire_audio::OpusPacket>>,
    Option<AudioStreamConfig>,
)> {
    if !session.has_capability(&Capability::Audio) {
        warn!("no Audio capability — not sending audio");
        return Ok((None, None, None, None));
    }
    let audio_config = AudioConfig::default();
    let (audio_capture, audio_frame_rx) = start_audio_capture(audio_config.clone())?;
    let (audio_encode, pkt_rx) = AudioEncodeThread::start(audio_config, audio_frame_rx)
        .context("failed to start audio encoder")?;
    let stream_config = AudioStreamConfig {
        sample_rate: 48_000,
        channel_count: 1,
        bitrate: 32_000,
    };
    info!("audio pipeline started (host)");
    Ok((
        Some(audio_capture),
        Some(audio_encode),
        Some(pkt_rx),
        Some(stream_config),
    ))
}

async fn start_guest_audio_sender(
    session: &NegotiatedSession,
    conn: Arc<pairwire_transport::PairwireConnection>,
) -> Result<GuestAudioSenderParts> {
    if !session.has_capability(&Capability::Audio) {
        return Ok((None, None, None));
    }

    // Give the host a short grace period to finish its control-plane handshake
    // and start its datagram router before the guest begins sending microphone
    // datagrams. Without this, a same-machine/two-process test can race: the
    // guest starts capture immediately after its local handshake completes while
    // the host is still unwinding from the handshake path.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let audio_config = AudioConfig::default();
    let (audio_capture, audio_frame_rx) = start_audio_capture(audio_config.clone())?;
    let (audio_encode, mut guest_opus_rx) = AudioEncodeThread::start(audio_config, audio_frame_rx)
        .context("failed to start guest audio encoder")?;

    let send_task = tokio::spawn(async move {
        while let Some(pkt) = guest_opus_rx.recv().await {
            if let Err(error) =
                datagram_router::send_tagged(conn.raw_conn(), TAG_GUEST_AUDIO, &pkt.data)
            {
                warn!(%error, "guest audio datagram send failed");
                break;
            }
        }
    });

    info!("guest audio sender started");
    Ok((Some(audio_capture), Some(audio_encode), Some(send_task)))
}

/// Spawn a task that polls the local cursor at ~60 Hz and sends [`CursorUpdate`]s to the
/// provided channel. The task exits when the channel send fails (receiver dropped) or when
/// [`pairwire_input::current_cursor_state`] returns an error.
///
/// `peer_id` is stamped onto every emitted update so the receiver can identify the sender.
/// Pass `frame_width`/`frame_height` of 0 to keep coordinates in display space (no scaling).
fn run_cursor_poll(
    cursor_update_tx: tokio::sync::mpsc::Sender<CursorUpdate>,
    peer_id: u16,
    frame_width: u16,
    frame_height: u16,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(16));
        let mut last_sent: Option<CursorUpdate> = None;

        loop {
            interval.tick().await;
            match pairwire_input::current_cursor_state() {
                Ok(((x, y), (display_width, display_height))) => {
                    let (x, y) = scale_cursor_to_frame(
                        x,
                        y,
                        display_width,
                        display_height,
                        frame_width,
                        frame_height,
                    );
                    let update = CursorUpdate {
                        peer_id,
                        x,
                        y,
                        visible: true,
                        click_animation: 0,
                    };

                    if last_sent.as_ref() == Some(&update) {
                        continue;
                    }

                    last_sent = Some(update.clone());
                    if cursor_update_tx.send(update).await.is_err() {
                        break;
                    }
                }
                Err(error) => {
                    warn!(%error, "cursor polling stopped");
                    break;
                }
            }
        }
    })
}

fn scale_cursor_to_frame(
    x: u16,
    y: u16,
    display_width: u16,
    display_height: u16,
    frame_width: u16,
    frame_height: u16,
) -> (u16, u16) {
    if display_width == 0 || display_height == 0 || frame_width == 0 || frame_height == 0 {
        return (x, y);
    }

    let scaled_x = ((u32::from(x).saturating_mul(u32::from(frame_width)))
        / u32::from(display_width))
    .min(u32::from(frame_width.saturating_sub(1)));
    let scaled_y = ((u32::from(y).saturating_mul(u32::from(frame_height)))
        / u32::from(display_height))
    .min(u32::from(frame_height.saturating_sub(1)));

    (
        u16::try_from(scaled_x).unwrap_or(frame_width.saturating_sub(1)),
        u16::try_from(scaled_y).unwrap_or(frame_height.saturating_sub(1)),
    )
}

/// Start screen capture — real (scap) when `capture` feature is enabled, fake otherwise.
///
/// The `show_cursor` flag is forwarded to the capture thread; toggling it at
/// runtime causes the thread to rebuild the underlying capturer so the cursor
/// appears or disappears in subsequent frames.
#[cfg(feature = "encode")]
fn start_capture(
    config: CaptureConfig,
    show_cursor: pairwire_capture::ShowCursorFlag,
) -> Result<(
    CaptureThread,
    std::sync::mpsc::Receiver<pairwire_capture::RawFrame>,
)> {
    #[cfg(feature = "capture")]
    {
        match CaptureThread::start(config.clone(), show_cursor.clone()) {
            Ok(pair) => return Ok(pair),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "real screen capture unavailable — using fake source. \
                     On macOS: grant Screen Recording permission in System Settings → Privacy."
                );
            }
        }
    }
    CaptureThread::start_fake(config, show_cursor).context("failed to start fake capture")
}

/// Start audio capture — real (cpal) when `audio` feature is enabled, fake sine otherwise.
fn start_audio_capture(
    config: AudioConfig,
) -> Result<(
    pairwire_audio::AudioCaptureThread,
    std::sync::mpsc::Receiver<pairwire_audio::AudioFrame>,
)> {
    #[cfg(feature = "audio")]
    {
        match AudioCaptureThread::start(config.clone()) {
            Ok(pair) => return Ok(pair),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "real audio capture unavailable — using fake source. \
                     On macOS: grant Microphone permission in System Settings → Privacy."
                );
            }
        }
    }
    AudioCaptureThread::start_fake(config).context("failed to start fake audio capture")
}

fn available_video_codecs() -> Vec<VideoCodec> {
    #[cfg(feature = "encode")]
    {
        vec![VideoCodec::H264]
    }
    #[cfg(not(feature = "encode"))]
    {
        Vec::new()
    }
}

fn host_capabilities() -> Vec<Capability> {
    #[cfg(feature = "audio")]
    {
        vec![
            Capability::Screen,
            Capability::Audio,
            Capability::RemoteControl,
            Capability::Annotations,
        ]
    }
    #[cfg(not(feature = "audio"))]
    {
        vec![
            Capability::Screen,
            Capability::RemoteControl,
            Capability::Annotations,
        ]
    }
}

fn guest_capabilities() -> Vec<Capability> {
    #[cfg(feature = "audio")]
    {
        vec![
            Capability::Screen,
            Capability::RemoteControl,
            Capability::Audio,
            Capability::Annotations,
        ]
    }
    #[cfg(not(feature = "audio"))]
    {
        vec![
            Capability::Screen,
            Capability::RemoteControl,
            Capability::Annotations,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::{run_cursor_poll, scale_cursor_to_frame};

    #[test]
    fn scale_cursor_maps_into_frame_space() {
        assert_eq!(
            scale_cursor_to_frame(1280, 720, 2560, 1440, 1920, 1080),
            (960, 540)
        );
    }

    #[test]
    fn scale_cursor_clamps_to_frame_bounds() {
        assert_eq!(
            scale_cursor_to_frame(u16::MAX, u16::MAX, 2560, 1440, 1920, 1080),
            (1919, 1079)
        );
    }

    #[test]
    fn scale_cursor_origin() {
        assert_eq!(scale_cursor_to_frame(0, 0, 2560, 1440, 1920, 1080), (0, 0));
    }

    #[test]
    fn scale_cursor_zero_display_returns_identity() {
        // Zero display dimensions should return input as-is (no division by zero).
        assert_eq!(
            scale_cursor_to_frame(100, 200, 0, 0, 1920, 1080),
            (100, 200)
        );
    }

    #[test]
    fn scale_cursor_zero_frame_returns_identity() {
        assert_eq!(
            scale_cursor_to_frame(100, 200, 2560, 1440, 0, 0),
            (100, 200)
        );
    }

    #[test]
    fn scale_cursor_same_dimensions() {
        // Same display and frame size — should be identity.
        assert_eq!(
            scale_cursor_to_frame(500, 300, 1920, 1080, 1920, 1080),
            (500, 300)
        );
    }

    #[test]
    fn scale_cursor_retina_downscale() {
        // Retina: display 3024x1964, frame 1512x982 (2x downscale)
        assert_eq!(
            scale_cursor_to_frame(1512, 982, 3024, 1964, 1512, 982),
            (756, 491)
        );
    }

    // current_cursor_state requires a real CGEvent context on macOS and cannot
    // be meaningfully exercised in a unit test environment. This smoke test
    // verifies that run_cursor_poll compiles, constructs without panicking, and
    // exits cleanly when the receiving end of the channel is dropped.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn run_cursor_poll_exits_on_channel_close() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx);
        let handle = run_cursor_poll(tx, 1, 0, 0);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        handle.abort();
        let _ = handle.await;
    }
}
