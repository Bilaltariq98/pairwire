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
    AudioStreamConfig, CursorReceiver, CursorSender, DatagramChannels, DrawingReceiver,
    DrawingSender, MoqSessionSubscriber, NegotiatedSession, QualityMonitor, TAG_GUEST_AUDIO,
    Transport, datagram_router, guest_handshake, host_handshake, local_handshake,
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
pub async fn run_host(
    args: HostArgs,
    invite_tx: Option<tokio::sync::oneshot::Sender<String>>,
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

    // Accept the input bidi stream opened by the guest (before moq-lite starts).
    // Auto-grant FullControl when allow_remote_control is true (default: false).
    let mut clipboard_recv_handle = None;
    let input_recv_handle = if session.has_capability(&Capability::RemoteControl) {
        match conn.accept_bi().await {
            Ok(Some((_input_send, input_recv))) => {
                // Clipboard messages arriving on the input stream are routed to
                // ClipboardReceiver, which writes them to the host OS clipboard.
                let (clipboard_sync_tx, clipboard_sync_rx) =
                    tokio::sync::mpsc::channel(8);
                let cb_receiver = ClipboardReceiver::new(clipboard_sync_rx);
                clipboard_recv_handle = Some(cb_receiver.spawn());

                let receiver = InputReceiver::new(input_recv)
                    .with_clipboard(clipboard_sync_tx);
                let handle = receiver.spawn();
                if args.allow_remote_control {
                    handle.set_permission(pairwire_input::InputPermission::FullControl);
                    info!("input receiver started — permission: FullControl (auto-granted)");
                } else {
                    info!(
                        "input receiver started — permission: Denied (use --allow-remote-control to enable)"
                    );
                }
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
                host_draw_point_tx = Some(draw_pt_tx);
                // draw_event_tx unused on host for now — host has no viewer to
                // capture drawing input from. Kept for future host-side annotation.
                drop(draw_event_tx);
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
    let cursor_poll_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(16));
        let mut last_sent = None;

        loop {
            interval.tick().await;
            match pairwire_input::current_cursor_state() {
                Ok(((x, y), (display_width, display_height))) => {
                    let (x, y) = scale_cursor_to_frame(
                        x,
                        y,
                        display_width,
                        display_height,
                        cursor_frame_width,
                        cursor_frame_height,
                    );
                    let update = CursorUpdate {
                        peer_id: 0,
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
                    warn!(%error, "host cursor polling stopped");
                    break;
                }
            }
        }
    });

    let (_host_cursor_handle, _host_cursor_watch) = CursorReceiver::spawn(cursor_chan_rx);

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
            let (capture_thread, frame_rx) = start_capture(capture_config.clone())?;

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
    quality_handle.stop().await;
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
pub async fn run_join(
    args: JoinArgs,
    frame_tx: watch::Sender<Option<DecodedFrame>>,
    cursor_tx: watch::Sender<Option<CursorUpdate>>,
    drawing_tx: watch::Sender<Vec<pairwire_protocol::drawing::DrawStroke>>,
    mut drawing_event_rx: tokio::sync::mpsc::Receiver<pairwire_transport::DrawingEvent>,
    mute_rx: watch::Receiver<bool>,
    quality_tx: watch::Sender<pairwire_transport::QualityTier>,
    mut viewer_input_rx: tokio::sync::mpsc::Receiver<pairwire_protocol::input::InputMessage>,
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

    // Quality monitor on the guest side — drives the HUD quality dot.
    // Uses a dummy CaptureConfig since the guest doesn't encode; we only need the tier.
    let guest_quality_monitor = QualityMonitor::new(
        Arc::new(conn.raw_conn().clone()),
        CaptureConfig::default(),
    );
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
    router.stop().await;
    cursor_forward_task.abort();
    let _ = cursor_forward_task.await;
    cursor_recv_handle.stop().await;
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
#[cfg(feature = "encode")]
fn start_capture(
    config: CaptureConfig,
) -> Result<(
    CaptureThread,
    std::sync::mpsc::Receiver<pairwire_capture::RawFrame>,
)> {
    #[cfg(feature = "capture")]
    {
        match CaptureThread::start(config.clone()) {
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
    CaptureThread::start_fake(config).context("failed to start fake capture")
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

#[cfg(test)]
mod tests {
    use super::scale_cursor_to_frame;

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
}
