//! Session orchestration — wires all subsystems together for host and join modes.
//!
//! `run_host`: bind transport, start capture, accept connection, handshake, stream frames
//! `run_join`: connect to host, handshake, receive frames, decode, forward to viewer
//!
//! Encode/decode require the `encode` feature. Without it, the pipeline stubs out
//! gracefully with a warning after the handshake completes.

use std::sync::Arc;

use anyhow::{Context, Result};
use pairwire_audio::{
    AudioCaptureThread, AudioDecodeThread, AudioEncodeThread, AudioPlaybackThread,
};
use pairwire_capture::{CaptureThread, DecodedFrame};
use pairwire_core::{
    SessionId,
    config::{AudioConfig, CaptureConfig, VideoEncoder},
};
use pairwire_protocol::{Capability, CursorUpdate};
#[cfg(feature = "encode")]
use pairwire_transport::{CursorReceiver, ScreenSender};
use pairwire_transport::{
    CursorSender, MoqAudioPublisher, MoqAudioSubscriber, NegotiatedSession, ScreenReceiver,
    Transport, guest_handshake, host_handshake, local_handshake,
};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::cli::{HostArgs, JoinArgs};

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

    let session_id = SessionId(rand::random());
    let invite = transport
        .invite_link(session_id)
        .await
        .context("failed to generate invite link")?;

    println!("\n  Invite link:\n\n    {invite}\n");
    println!("  Waiting for guest to connect…\n");

    if let Some(tx) = invite_tx {
        // Ignore send error — receiver may have been dropped (e.g., no tray feature).
        let _ = tx.send(invite.to_string());
    }

    let capture_config = CaptureConfig {
        fps: args.fps,
        width: args.width,
        height: args.height,
        preferred_encoder: VideoEncoder::H264Software,
        ..Default::default()
    };

    // Use real screen capture when the `capture` feature is enabled,
    // falling back to the fake source otherwise.
    let (capture_thread, _frame_rx) = start_capture(capture_config.clone())?;
    #[cfg(feature = "encode")]
    let frame_rx = _frame_rx;

    let conn = Arc::new(
        transport
            .accept()
            .await
            .context("accept failed")?
            .ok_or_else(|| anyhow::anyhow!("endpoint closed before connection"))?,
    );
    info!(peer = %conn.remote_id(), "guest connected");

    // Perform handshake before starting media.
    let session = perform_host_handshake(&conn, &capture_config).await?;
    info!(
        capabilities = ?session.capabilities,
        video_codec = ?session.video_codec,
        "handshake complete"
    );

    // Start audio pipeline if both peers support it.
    let audio_handles = if session.has_capability(&pairwire_protocol::Capability::Audio) {
        let audio_config = AudioConfig::default();
        let (audio_capture, audio_frame_rx) = start_audio_capture(audio_config.clone())?;
        let (audio_encode, audio_pkt_rx) = AudioEncodeThread::start(audio_config, audio_frame_rx)
            .context("failed to start audio encoder")?;
        let audio_pub = MoqAudioPublisher::new(Arc::clone(&conn));
        let audio_pub_handle = audio_pub.spawn(audio_pkt_rx);
        info!("audio pipeline started (host)");
        Some((audio_capture, audio_encode, audio_pub_handle))
    } else {
        warn!("no Audio capability — not sending audio");
        None
    };

    // Encode + send only when FFmpeg is available and codec was negotiated.
    #[cfg(feature = "encode")]
    {
        use pairwire_capture::EncodeThread;

        if session.video_codec.is_none() {
            warn!("no common video codec — not sending frames");
        } else {
            let (encode_thread, packet_rx) =
                EncodeThread::start(capture_config, frame_rx).context("failed to start encoder")?;

            let cursor_recv = CursorReceiver::new(Arc::clone(&conn));
            let (cursor_handle, _) = cursor_recv.spawn();

            let screen_sender = ScreenSender::new(Arc::clone(&conn));
            let sender_handle = screen_sender.spawn(packet_rx);

            conn.closed().await;
            info!("guest disconnected");

            sender_handle.stop().await;
            cursor_handle.stop().await;
            encode_thread.stop();
        }
    }

    #[cfg(not(feature = "encode"))]
    {
        warn!("encode feature not enabled — handshake done but not sending frames");
        conn.closed().await;
        info!("guest disconnected");
    }

    if let Some((audio_capture, audio_encode, audio_pub_handle)) = audio_handles {
        audio_pub_handle.stop().await;
        audio_encode.stop();
        audio_capture.stop();
    }

    capture_thread.stop();
    transport.close().await;
    Ok(())
}

/// Run as guest: connect to host, handshake, receive and decode frames, forward to viewer.
pub async fn run_join(
    args: JoinArgs,
    frame_tx: watch::Sender<Option<DecodedFrame>>,
    _cursor_tx: watch::Sender<Option<CursorUpdate>>,
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
    let session = perform_guest_handshake(&conn).await?;
    info!(
        capabilities = ?session.capabilities,
        video_codec = ?session.video_codec,
        "handshake complete"
    );

    let (pkt_tx, _pkt_rx) = std::sync::mpsc::sync_channel::<pairwire_capture::EncodedPacket>(8);
    #[cfg(feature = "encode")]
    let pkt_rx = _pkt_rx;

    let screen_recv = ScreenReceiver::new(Arc::clone(&conn));
    let recv_handle = screen_recv.spawn(pkt_tx);

    // Keep cursor_update_tx alive for the duration of the session.
    let (cursor_update_tx, cursor_update_rx) = tokio::sync::mpsc::channel(64);
    let cursor_send = CursorSender::new(Arc::clone(&conn));
    let cursor_send_handle = cursor_send.spawn(cursor_update_rx);
    let _cursor_update_tx = cursor_update_tx;

    // Start audio receive pipeline if both peers support it.
    let audio_handles = if session.has_capability(&pairwire_protocol::Capability::Audio) {
        let audio_config = AudioConfig::default();
        let (audio_pkt_tx, audio_pkt_rx) = tokio::sync::mpsc::channel(32);
        let audio_sub = MoqAudioSubscriber::new(Arc::clone(&conn));
        let audio_sub_handle = audio_sub.spawn(audio_pkt_tx);
        let (audio_decode, audio_frame_rx) =
            AudioDecodeThread::start(audio_config.clone(), audio_pkt_rx)
                .context("failed to start audio decoder")?;
        let audio_playback = AudioPlaybackThread::start(audio_config, audio_frame_rx)
            .context("failed to start audio playback")?;
        info!("audio pipeline started (guest)");
        Some((audio_sub_handle, audio_decode, audio_playback))
    } else {
        warn!("no Audio capability — not receiving audio");
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

    if let Some((audio_sub_handle, audio_decode, audio_playback)) = audio_handles {
        audio_sub_handle.stop().await;
        audio_decode.stop();
        audio_playback.stop();
    }

    recv_handle.stop().await;
    cursor_send_handle.stop().await;
    transport.close().await;
    Ok(())
}

/// Open the control stream and perform the host-side handshake.
async fn perform_host_handshake(
    conn: &pairwire_transport::PairwireConnection,
    config: &CaptureConfig,
) -> Result<NegotiatedSession> {
    let (send, recv) = conn
        .open_control_stream()
        .await
        .context("failed to open control stream")?;

    #[allow(clippy::cast_possible_truncation)] // display dimensions are always < 65535
    let local = local_handshake(
        host_capabilities(),
        config.width as u16,
        config.height as u16,
        100, // scale factor — real value from display API in Phase 3
    );

    let (_send, _recv, session) = host_handshake(send, recv, local)
        .await
        .context("handshake failed")?;

    // _send/_recv kept for future heartbeat/control messages (P2.5)
    Ok(session)
}

/// Accept the control stream and perform the guest-side handshake.
async fn perform_guest_handshake(
    conn: &pairwire_transport::PairwireConnection,
) -> Result<NegotiatedSession> {
    let (send, recv) = conn
        .accept_bi()
        .await
        .context("failed to accept control stream")?
        .ok_or_else(|| anyhow::anyhow!("connection closed before control stream"))?;

    let local = local_handshake(
        guest_capabilities(),
        0, // guest doesn't share a display
        0,
        0,
    );

    let (_send, _recv, session) = guest_handshake(send, recv, local)
        .await
        .context("handshake failed")?;

    Ok(session)
}

/// Start screen capture — real (scap) when `capture` feature is enabled, fake otherwise.
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
                // Permission or hardware failure — fall through to fake source with a clear message.
                tracing::warn!(
                    error = %e,
                    "real screen capture unavailable — using fake source. \
                     On macOS: grant Screen Recording permission in System Settings → Privacy."
                );
            }
        }
    }
    // Fake source: always available, used when capture feature is off or permission denied.
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

fn host_capabilities() -> Vec<Capability> {
    #[cfg(feature = "audio")]
    {
        vec![Capability::Screen, Capability::Audio]
    }
    #[cfg(not(feature = "audio"))]
    {
        vec![Capability::Screen]
    }
}

fn guest_capabilities() -> Vec<Capability> {
    #[cfg(feature = "audio")]
    {
        vec![
            Capability::Screen,
            Capability::RemoteControl,
            Capability::Audio,
        ]
    }
    #[cfg(not(feature = "audio"))]
    {
        vec![Capability::Screen, Capability::RemoteControl]
    }
}
