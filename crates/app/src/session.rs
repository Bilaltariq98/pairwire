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

use std::sync::Arc;

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
use pairwire_transport::{
    AudioStreamConfig, ControlChannel, CursorReceiver, CursorSender, DatagramChannels,
    DrawingReceiver, DrawingSender, MoqSessionSubscriber, NegotiatedSession, QualityMonitor,
    TAG_GUEST_AUDIO, Transport, datagram_router, guest_handshake, host_handshake, local_handshake,
};
#[cfg(feature = "encode")]
use pairwire_transport::{MoqSessionPublisher, VideoStreamConfig};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::cli::{HostArgs, JoinArgs};

type GuestAudioSenderParts = (
    Option<AudioCaptureThread>,
    Option<AudioEncodeThread>,
    Option<tokio::task::JoinHandle<()>>,
);

/// Run as host: capture screen, accept one connection, handshake, stream frames.
///
/// `invite_tx` receives the invite URL as soon as it is generated, before the
/// blocking `accept()` call. Pass `None` if you don't need the URL out-of-band.
///
/// `tray_action_rx` receives `ControlAction` events from the tray menu (Grant /
/// Revoke). Pass `None` when running without a tray (tests, non-tray builds).
pub async fn run_host(
    args: HostArgs,
    invite_tx: Option<tokio::sync::oneshot::Sender<String>>,
    tray_action_rx: Option<tokio::sync::mpsc::Receiver<crate::control_state::ControlAction>>,
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

    let conn = Arc::new(
        transport
            .accept()
            .await
            .context("accept failed")?
            .ok_or_else(|| anyhow::anyhow!("endpoint closed before connection"))?,
    );
    info!(peer = %conn.remote_id(), "guest connected");

    // Perform handshake before starting media.
    let session = perform_host_handshake(&conn, &capture_config, session_token).await?;
    info!(
        capabilities = ?session.capabilities,
        video_codec = ?session.video_codec,
        "handshake complete"
    );

    // Persistent control channel — must be the first post-handshake bidi accept so
    // the ordinal position matches the guest's `open_control_stream` sequence below.
    let (host_ctrl_outbound_tx, host_ctrl_outbound_rx) =
        tokio::sync::mpsc::channel::<pairwire_protocol::ControlMessage>(32);
    let (ctrl_send, ctrl_recv) = conn
        .accept_bi()
        .await
        .context("failed to accept control channel stream")?
        .ok_or_else(|| anyhow::anyhow!("connection closed before control channel"))?;
    let host_ctrl_handle = ControlChannel::spawn(ctrl_send, ctrl_recv, host_ctrl_outbound_rx);
    info!("control channel accepted");

    // Accept the input bidi stream opened by the guest (before moq-lite starts).
    // Permission starts as Denied; the host control-state task grants FullControl
    // when a ControlRequest arrives (auto_grant=true) or when the host clicks
    // "Allow Remote Control" in the tray.
    let mut clipboard_recv_handle = None;
    let input_recv_handle = if session.has_capability(&Capability::RemoteControl) {
        match conn.accept_bi().await {
            Ok(Some((_input_send, input_recv))) => {
                // Clipboard messages arriving on the input stream are routed to
                // ClipboardReceiver, which writes them to the host OS clipboard.
                let (clipboard_sync_tx, clipboard_sync_rx) = tokio::sync::mpsc::channel(8);
                let cb_receiver = ClipboardReceiver::new(clipboard_sync_rx);
                clipboard_recv_handle = Some(cb_receiver.spawn());

                let receiver = InputReceiver::new(input_recv).with_clipboard(clipboard_sync_tx);
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

    // Clipboard sync is currently guest→host only: the guest's ClipboardSender pushes
    // InputMessage::Clipboard onto the input stream; InputReceiver routes it to
    // ClipboardReceiver above. Host→guest requires a reverse send path (bidi input
    // stream or a separate stream) — deferred.

    // Accept the annotation bidi stream opened by the guest (before moq-lite starts).
    // Used for DrawStroke / DrawClear messages in both directions.
    // draw_point_tx feeds live DrawPoint datagrams from the router into DrawingReceiver.
    let mut host_drawing_send_handle = None;
    let mut host_drawing_recv_handle = None;
    let mut _host_draw_event_tx = None;
    let mut host_draw_point_tx = None;

    if session.has_capability(&Capability::Annotations) {
        match conn.accept_bi().await {
            Ok(Some((annot_send, annot_recv))) => {
                let (draw_event_tx, draw_event_rx) = tokio::sync::mpsc::channel(32);
                let (draw_pt_tx, draw_pt_rx) = tokio::sync::mpsc::channel(64);
                let sender = DrawingSender::new(Arc::clone(&conn), annot_send);
                let (recv_handle, _strokes_rx, _live_rx) =
                    DrawingReceiver::new(annot_recv).spawn(draw_pt_rx);
                host_drawing_send_handle = Some(sender.spawn(draw_event_rx));
                host_drawing_recv_handle = Some(recv_handle);
                _host_draw_event_tx = Some(draw_event_tx);
                host_draw_point_tx = Some(draw_pt_tx);
                // draw_event_tx is kept alive so the host's annotation send half
                // stays open even though the headless host has no local drawing UI yet.
                info!("annotation stream accepted");
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

    let (host_cursor_recv_handle, mut host_cursor_watch) = CursorReceiver::spawn(cursor_chan_rx);
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
        start_host_audio(&session)?;
    #[cfg(feature = "encode")]
    let (audio_pkt_rx, audio_stream_config) = (_audio_pkt_rx, _audio_stream_config);

    let mut guest_audio_decode_handle = None;
    let mut guest_audio_playback_handle = None;
    let guest_audio_forward_task = if session.has_capability(&Capability::Audio) {
        let audio_config = AudioConfig::default();
        let (guest_audio_decode_tx, guest_audio_decode_rx) = tokio::sync::mpsc::channel(64);
        let (guest_audio_decode, guest_audio_frame_rx) =
            AudioDecodeThread::start(audio_config.clone(), guest_audio_decode_rx)
                .context("failed to start guest audio decoder")?;
        let guest_audio_playback = AudioPlaybackThread::start(audio_config, guest_audio_frame_rx)
            .context("failed to start guest audio playback")?;
        guest_audio_decode_handle = Some(guest_audio_decode);
        guest_audio_playback_handle = Some(guest_audio_playback);
        let task = tokio::spawn(async move {
            const FRAME_DURATION_US: u64 = 20_000; // 20ms Opus frames
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

    // Host control-state machine: receives ControlRequest from the guest (via the
    // ControlChannel inbound broadcast) and tray actions (Grant / Revoke) from
    // the main thread, then updates the AtomicPermission and sends ControlGrant /
    // ControlRevoke back to the guest.
    let host_perm_handle = input_recv_handle
        .as_ref()
        .map(pairwire_input::InputReceiverHandle::permission_handle);

    // tray_action_rx is provided by run_host's caller (see main.rs).
    // When no tray is attached, we synthesize an immediately-closed receiver —
    // the sender is dropped at the end of this expression so the receiver yields
    // `None` on first poll, which sets the `tray_closed` guard in
    // `host_ctrl_state_task_fn` and disables the tray arm of the select.
    let (host_tray_default_tx, host_tray_default_rx) =
        tokio::sync::mpsc::channel::<crate::control_state::ControlAction>(1);
    let mut host_tray_action_rx = if let Some(rx) = tray_action_rx {
        rx
    } else {
        drop(host_tray_default_tx);
        host_tray_default_rx
    };

    // Cursor-visibility flag — starts visible (true); flipped to false when the
    // guest is granted control and back to true on revoke. All clones share the
    // underlying AtomicBool, so the control-state task and capture thread stay
    // in sync without any additional locking.
    let capture_show_cursor = pairwire_capture::ShowCursorFlag::new(true);

    let auto_grant = args.allow_remote_control;
    let host_ctrl_outbound_for_task = host_ctrl_outbound_tx.clone();
    let mut host_ctrl_inbound_for_task = host_ctrl_handle.subscribe();
    let host_capture_show_cursor = capture_show_cursor.clone();

    let host_ctrl_state_task = tokio::spawn(async move {
        host_ctrl_state_task_fn(
            auto_grant,
            &mut host_ctrl_inbound_for_task,
            &mut host_tray_action_rx,
            &host_ctrl_outbound_for_task,
            &host_perm_handle,
            host_capture_show_cursor,
        )
        .await;
    });

    // Encode + unified moq publish (screen + audio in one session).
    #[cfg(feature = "encode")]
    {
        use pairwire_capture::EncodeThread;

        if session.video_codec.is_none() {
            warn!("no common video codec — not sending frames");
        } else {
            // Start capture only when the encoder/publisher are about to consume
            // frames. Starting it while waiting for a guest just fills the small
            // bounded channel and emits noisy "dropping frame" logs.
            let (capture_thread, frame_rx) =
                start_capture(capture_config.clone(), capture_show_cursor.clone())?;

            let (encode_thread, video_pkt_rx) = EncodeThread::start_with_quality(
                capture_config.clone(),
                frame_rx,
                Some(quality_rx),
            )
            .context("failed to start encoder")?;

            let video_stream_config = VideoStreamConfig {
                width: capture_config.width,
                height: capture_config.height,
                fps: f64::from(capture_config.fps),
                bitrate: 5_000_000,
            };

            let publisher = MoqSessionPublisher::new(Arc::clone(&conn));
            let pub_handle = publisher.spawn(
                video_stream_config,
                audio_stream_config,
                video_pkt_rx,
                audio_pkt_rx,
            );

            conn.closed().await;
            info!("guest disconnected");

            pub_handle.stop().await;
            encode_thread.stop();
            capture_thread.stop();
        }
    }

    #[cfg(not(feature = "encode"))]
    {
        warn!("encode feature not enabled — handshake done but not sending frames");
        conn.closed().await;
        info!("guest disconnected");
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
    if let Some(handle) = host_drawing_send_handle {
        handle.stop().await;
    }
    if let Some(handle) = host_drawing_recv_handle {
        handle.stop().await;
    }
    if let Some(handle) = clipboard_recv_handle {
        handle.stop().await;
    }
    if let Some(handle) = input_recv_handle {
        handle.stop().await;
    }
    if let Some(task) = guest_audio_forward_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(decode) = guest_audio_decode_handle {
        decode.stop();
    }
    if let Some(playback) = guest_audio_playback_handle {
        playback.stop();
    }
    if let Some(encode) = audio_encode_handle {
        encode.stop();
    }
    if let Some(capture) = audio_capture_handle {
        capture.stop();
    }

    transport.close().await;
    Ok(())
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
#[allow(clippy::too_many_arguments)]
pub async fn run_join(
    args: JoinArgs,
    frame_tx: watch::Sender<Option<DecodedFrame>>,
    cursor_tx: watch::Sender<Option<CursorUpdate>>,
    drawing_tx: watch::Sender<Vec<pairwire_protocol::drawing::DrawStroke>>,
    drawing_event_rx: tokio::sync::mpsc::Receiver<pairwire_transport::DrawingEvent>,
    mute_rx: watch::Receiver<bool>,
    quality_tx: watch::Sender<pairwire_transport::QualityTier>,
    mut viewer_input_rx: tokio::sync::mpsc::Receiver<pairwire_protocol::input::InputMessage>,
    control_state_tx: watch::Sender<crate::control_state::RuntimeControlState>,
    mut control_request_rx: tokio::sync::mpsc::Receiver<pairwire_protocol::ControlMessage>,
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

    // Give the host a short turn to finish its handshake path and enter its
    // post-handshake accept loop before the guest opens annotation/moq streams.
    // On same-machine runs, opening follow-up bidi streams immediately after
    // the guest handshake can intermittently leave the host stuck waiting for
    // the handshake response while the viewer sits at "Connecting…".
    // TODO: replace this timing guard with an explicit host-ready control ack
    // once the control plane has a persistent session-management stream.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    // Persistent control channel — must be the first post-handshake bidi open so
    // the ordinal position matches the host's `accept_bi` sequence above.
    // T5 will wire up a real consumer for `_guest_ctrl_inbound`.
    // guest_ctrl_outbound_tx is used below to flush the stream; T5 will add more senders.
    let (guest_ctrl_outbound_tx, guest_ctrl_outbound_rx) =
        tokio::sync::mpsc::channel::<pairwire_protocol::ControlMessage>(32);
    let (ctrl_send, ctrl_recv) = conn
        .open_control_stream()
        .await
        .context("failed to open control channel stream")?;
    let guest_ctrl_handle = ControlChannel::spawn(ctrl_send, ctrl_recv, guest_ctrl_outbound_rx);

    // Force the QUIC bidi stream open by sending an initial message — open_bi
    // alone doesn't announce the stream to the remote's accept_bi until the first
    // STREAM frame is written. SetSharing is used as a no-op flush for now;
    // T5 can introduce a dedicated ChannelReady variant later.
    guest_ctrl_outbound_tx
        .send(pairwire_protocol::ControlMessage::SetSharing {
            screen: true,
            audio: true,
        })
        .await
        .ok();
    info!("control channel opened");

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
                let (recv_handle, strokes_rx, _live_rx) =
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

    #[cfg(feature = "encode")]
    {
        use pairwire_capture::DecodeThread;

        if session.video_codec.is_none() {
            warn!("no common video codec — not decoding frames");
            conn.closed().await;
            info!("host disconnected");
        } else {
            let (decode_thread, mut decoded_rx) =
                DecodeThread::start(pkt_rx).context("failed to start decoder")?;

            tokio::spawn(async move {
                while let Some(frame) = decoded_rx.recv().await {
                    let _ = frame_tx.send(Some(frame));
                }
            });

            conn.closed().await;
            info!("host disconnected");
            decode_thread.stop();
        }
    }

    #[cfg(not(feature = "encode"))]
    {
        warn!("encode feature not enabled — handshake done but not decoding frames");
        conn.closed().await;
        info!("host disconnected");
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
            Capability::Annotations,
        ]
    }
    #[cfg(not(feature = "audio"))]
    {
        vec![Capability::Screen, Capability::Annotations]
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

/// Update the `AtomicPermission` to match the current control state.
///
/// Called whenever the host control state transitions so that the input
/// receiver immediately reflects the new gating without a lock.
fn apply_permission(
    perm: &Option<Arc<pairwire_input::AtomicPermission>>,
    state: crate::control_state::RuntimeControlState,
) {
    if let Some(p) = perm {
        p.set(state.input_permission());
    }
}

/// Host control-state machine loop.
///
/// Drives `RuntimeControlState` from inbound `ControlMessage`s (guest-initiated)
/// and tray `ControlAction`s (host-initiated). Updates `AtomicPermission` in-place
/// and sends `ControlGrant` / `ControlRevoke` back to the guest.
///
/// Exits when both the inbound broadcast channel and the tray action channel
/// are closed (i.e., after session teardown).
///
/// Extracted as a free function so tests can drive it without a full session.
pub(crate) async fn host_ctrl_state_task_fn(
    auto_grant: bool,
    inbound: &mut tokio::sync::broadcast::Receiver<pairwire_protocol::ControlMessage>,
    tray_rx: &mut tokio::sync::mpsc::Receiver<crate::control_state::ControlAction>,
    outbound_tx: &tokio::sync::mpsc::Sender<pairwire_protocol::ControlMessage>,
    perm_handle: &Option<Arc<pairwire_input::AtomicPermission>>,
    show_cursor: pairwire_capture::ShowCursorFlag,
) {
    use crate::control_state::{ControlAction, RuntimeControlState};
    use pairwire_protocol::{ControlGrant, ControlMessage, ControlRevoke};

    let mut state = RuntimeControlState::HostActive;
    // Track whether each source has been permanently closed.
    // The task exits only when the inbound control channel closes;
    // the tray channel is optional and its closure is not a terminal condition.
    let mut tray_closed = false;
    // Remember the most-recent requesting peer so tray-initiated Grant/Revoke
    // can address the right guest. Falls back to 1 for unsolicited grants where
    // the host clicks "Allow" before the guest has sent a ControlRequest.
    let mut pending_peer_id: Option<u16> = None;

    loop {
        tokio::select! {
            msg = inbound.recv() => {
                match msg {
                    Ok(ControlMessage::ControlRequest(req)) => {
                        state.apply(ControlAction::Request);
                        pending_peer_id = Some(req.peer_id);
                        tracing::info!(peer_id = req.peer_id, "guest requested control");
                        if auto_grant {
                            state.apply(ControlAction::Grant);
                            apply_permission(perm_handle, state);
                            // Hide the host cursor so the guest's overlay arrow is
                            // the only cursor visible in the streamed frames.
                            show_cursor.set(false);
                            let _ = outbound_tx
                                .send(ControlMessage::ControlGrant(ControlGrant {
                                    peer_id: req.peer_id,
                                }))
                                .await;
                            tracing::info!("auto-granted control to guest");
                        }
                    }
                    Ok(_) => {} // other control messages not handled by host state machine
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!(n, "host ctrl inbound lagged — continuing");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            action = tray_rx.recv(), if !tray_closed => {
                match action {
                    Some(action) => {
                        // Use the most-recently seen requester; fall back to 1 for
                        // single-guest sessions where the host grants before any request.
                        let peer_id = pending_peer_id.unwrap_or(1);
                        state.apply(action);
                        apply_permission(perm_handle, state);
                        match action {
                            ControlAction::Grant => {
                                // Hide the host cursor while the guest has control.
                                show_cursor.set(false);
                                let _ = outbound_tx
                                    .send(ControlMessage::ControlGrant(ControlGrant {
                                        peer_id,
                                    }))
                                    .await;
                                tracing::info!("host granted control via tray");
                            }
                            ControlAction::Revoke => {
                                // Restore the host cursor now that the guest no longer
                                // has control.
                                show_cursor.set(true);
                                let _ = outbound_tx
                                    .send(ControlMessage::ControlRevoke(ControlRevoke {
                                        peer_id,
                                    }))
                                    .await;
                                tracing::info!("host revoked control via tray");
                            }
                            ControlAction::Request => {} // host never requests
                        }
                    }
                    None => {
                        // Tray sender dropped — disable this arm, continue on inbound.
                        tray_closed = true;
                    }
                }
            }
        }
    }
}

/// Guest control-state machine loop.
///
/// Mirrors `ControlGrant` / `ControlRevoke` messages sent by the host and
/// forwards the new [`RuntimeControlState`] to a watch channel so the viewer
/// can reflect the current control state without polling.
///
/// Exits when the inbound broadcast channel closes (i.e., after session teardown).
///
/// Extracted as a free function so tests can drive it without a full session.
pub(crate) async fn guest_ctrl_state_task_fn(
    mut inbound: tokio::sync::broadcast::Receiver<pairwire_protocol::ControlMessage>,
    state_tx: tokio::sync::watch::Sender<crate::control_state::RuntimeControlState>,
) {
    use crate::control_state::{ControlAction, RuntimeControlState};
    use pairwire_protocol::ControlMessage;

    let mut state = RuntimeControlState::HostActive;
    loop {
        match inbound.recv().await {
            Ok(ControlMessage::ControlGrant(_)) => {
                state.apply(ControlAction::Grant);
                let _ = state_tx.send(state);
                tracing::debug!("control granted by host");
            }
            Ok(ControlMessage::ControlRevoke(_)) => {
                state.apply(ControlAction::Revoke);
                let _ = state_tx.send(state);
                tracing::debug!("control revoked by host");
            }
            Ok(_) => {} // other messages not relevant to guest control state
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!(n, "guest ctrl inbound lagged — continuing");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        guest_ctrl_state_task_fn, host_ctrl_state_task_fn, run_cursor_poll, scale_cursor_to_frame,
    };

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

    /// Integration test: when auto_grant=true, a ControlRequest from the guest side
    /// causes the host state machine to emit ControlGrant, flip AtomicPermission
    /// to FullControl, and set the ShowCursorFlag to false — driven purely
    /// in-process via broadcast channel stubs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_auto_grant_on_control_request() {
        use std::sync::Arc;

        use pairwire_capture::ShowCursorFlag;
        use pairwire_input::AtomicPermission;
        use pairwire_protocol::{ControlGrant, ControlMessage, ControlRequest};
        use tokio::sync::mpsc;

        let (host_ctrl_outbound_tx, host_ctrl_outbound_rx) = mpsc::channel::<ControlMessage>(8);

        // perm handle: what we'll inspect after the task runs
        let atomic_perm = Arc::new(AtomicPermission::default());
        let perm_handle = Some(Arc::clone(&atomic_perm));

        // cursor visibility flag: starts visible, should flip to hidden on grant
        let show_cursor = ShowCursorFlag::new(true);
        let show_cursor_check = show_cursor.clone();

        let auto_grant = true;
        let (tray_action_tx, mut tray_action_rx) =
            mpsc::channel::<crate::control_state::ControlAction>(8);
        drop(tray_action_tx); // no tray actions in this test

        // Simulate the guest sending a ControlRequest to the host inbound.
        // Pre-load the ControlRequest then drop the sender so the task exits
        // when the channel becomes empty and closed.
        let (bcast_tx, mut host_inbound_rx) = tokio::sync::broadcast::channel::<ControlMessage>(8);
        let _ = bcast_tx.send(ControlMessage::ControlRequest(ControlRequest {
            peer_id: 1,
        }));
        drop(bcast_tx);

        host_ctrl_state_task_fn(
            auto_grant,
            &mut host_inbound_rx,
            &mut tray_action_rx,
            &host_ctrl_outbound_tx,
            &perm_handle,
            show_cursor,
        )
        .await;

        // The permission should have been flipped to FullControl.
        assert_eq!(
            atomic_perm.load_permission(),
            pairwire_input::InputPermission::FullControl,
            "permission should be FullControl after auto-grant"
        );

        // The cursor should have been hidden when control was granted.
        assert!(
            !show_cursor_check.get(),
            "show_cursor flag should be false after guest is granted control"
        );

        // The host should have sent a ControlGrant on the outbound channel.
        drop(host_ctrl_outbound_tx);
        let mut received_grant = false;
        let mut rx = host_ctrl_outbound_rx;
        while let Some(msg) = rx.recv().await {
            if matches!(
                msg,
                ControlMessage::ControlGrant(ControlGrant { peer_id: 1 })
            ) {
                received_grant = true;
            }
        }
        assert!(received_grant, "host should have sent ControlGrant");
    }

    /// Guest state machine: ControlGrant → GuestActive (watch emits); ControlRevoke → HostActive.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn guest_ctrl_state_task_grant_then_revoke() {
        use crate::control_state::RuntimeControlState;
        use pairwire_protocol::{ControlGrant, ControlMessage, ControlRevoke};

        let (bcast_tx, inbound_rx) = tokio::sync::broadcast::channel::<ControlMessage>(8);
        let (state_tx, state_rx) = tokio::sync::watch::channel(RuntimeControlState::HostActive);

        // Pre-load Grant then Revoke; drop the broadcast sender so the task exits
        // when it drains the channel and then receives the Closed error.
        let _ = bcast_tx.send(ControlMessage::ControlGrant(ControlGrant { peer_id: 1 }));
        let _ = bcast_tx.send(ControlMessage::ControlRevoke(ControlRevoke { peer_id: 1 }));
        drop(bcast_tx);

        // Run the guest state machine to completion (state_tx is moved in and dropped on exit).
        guest_ctrl_state_task_fn(inbound_rx, state_tx).await;

        // After Grant → Revoke, the final state should be back to HostActive.
        assert_eq!(
            *state_rx.borrow(),
            RuntimeControlState::HostActive,
            "state should return to HostActive after revoke"
        );

        // The watch sender was dropped when the task exited — has_changed() returns
        // Err(RecvError) in that case, which means the channel is closed but the
        // receiver can still read the last value (asserted above). This is expected.
        // We separately test the intermediate GuestActive transition via a spawn:
        let (bcast_tx2, inbound_rx2) = tokio::sync::broadcast::channel::<ControlMessage>(8);
        let (state_tx2, mut state_rx2) =
            tokio::sync::watch::channel(RuntimeControlState::HostActive);
        let _ = bcast_tx2.send(ControlMessage::ControlGrant(ControlGrant { peer_id: 1 }));
        drop(bcast_tx2);
        guest_ctrl_state_task_fn(inbound_rx2, state_tx2).await;
        // After a lone Grant the watch should emit GuestActive.
        assert_eq!(
            *state_rx2.borrow(),
            RuntimeControlState::GuestActive,
            "state should be GuestActive after lone Grant"
        );
    }
}
