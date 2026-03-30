//! Session orchestration — wires all subsystems together for host and join modes.
//!
//! `run_host`: bind transport, start capture, accept connection, stream frames
//! `run_join`: connect to host, receive frames, decode, forward to viewer
//!
//! Encode/decode require the `ffmpeg` feature on `pairwire-capture`.
//! Without it, the pipeline stubs out gracefully with a warning.

use std::sync::Arc;

use anyhow::{Context, Result};
use pairwire_capture::{CaptureThread, DecodedFrame};
use pairwire_core::{
    SessionId,
    config::{CaptureConfig, VideoEncoder},
};
use pairwire_protocol::CursorUpdate;
#[cfg(feature = "encode")]
use pairwire_transport::{CursorReceiver, ScreenSender};
use pairwire_transport::{CursorSender, ScreenReceiver, Transport};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::cli::{HostArgs, JoinArgs};

/// Run as host: capture screen, accept one connection, stream frames.
pub async fn run_host(args: HostArgs) -> Result<()> {
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

    let capture_config = CaptureConfig {
        fps: args.fps,
        width: args.width,
        height: args.height,
        preferred_encoder: VideoEncoder::H264Software,
        ..Default::default()
    };

    let (capture_thread, _frame_rx) =
        CaptureThread::start_fake(capture_config.clone()).context("failed to start capture")?;
    #[cfg(feature = "encode")]
    let frame_rx = _frame_rx;

    // Encode + send only when FFmpeg is available
    #[cfg(feature = "encode")]
    {
        use pairwire_capture::EncodeThread;
        let (encode_thread, packet_rx) =
            EncodeThread::start(capture_config, frame_rx).context("failed to start encoder")?;

        let conn = Arc::new(
            transport
                .accept()
                .await
                .context("accept failed")?
                .ok_or_else(|| anyhow::anyhow!("endpoint closed before connection"))?,
        );
        info!(peer = %conn.remote_id(), "guest connected");

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

    #[cfg(not(feature = "encode"))]
    {
        warn!("encode feature not enabled — accepting connection but not sending frames");
        let conn = Arc::new(
            transport
                .accept()
                .await
                .context("accept failed")?
                .ok_or_else(|| anyhow::anyhow!("endpoint closed before connection"))?,
        );
        info!(peer = %conn.remote_id(), "guest connected (no-encode mode)");
        conn.closed().await;
        info!("guest disconnected");
    }

    capture_thread.stop();
    transport.close().await;
    Ok(())
}

/// Run as guest: connect to host, receive and decode frames, forward to viewer.
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

    let (pkt_tx, _pkt_rx) = std::sync::mpsc::sync_channel::<pairwire_capture::EncodedPacket>(8);
    #[cfg(feature = "encode")]
    let pkt_rx = _pkt_rx;

    let screen_recv = ScreenReceiver::new(Arc::clone(&conn));
    let recv_handle = screen_recv.spawn(pkt_tx);

    // Keep cursor_update_tx alive for the duration of the session.
    // Dropping it immediately causes CursorSender to exit before the connection closes.
    let (cursor_update_tx, cursor_update_rx) = tokio::sync::mpsc::channel(64);
    let cursor_send = CursorSender::new(Arc::clone(&conn));
    let cursor_send_handle = cursor_send.spawn(cursor_update_rx);
    // cursor_update_tx will be wired to mouse input in Phase 2; hold it here for now.
    let _cursor_update_tx = cursor_update_tx;

    #[cfg(feature = "encode")]
    {
        use pairwire_capture::DecodeThread;
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

    #[cfg(not(feature = "encode"))]
    {
        warn!("encode feature not enabled — connected but not decoding frames");
        conn.closed().await;
        info!("host disconnected");
    }

    recv_handle.stop().await;
    cursor_send_handle.stop().await;
    transport.close().await;
    Ok(())
}
