//! Screen frame sender and receiver over QUIC streams.
//!
//! Protocol (per docs/05-concrete-architecture.md):
//! - Each keyframe opens a NEW unidirectional QUIC stream (moq-lite group model).
//! - Delta frames are written to the current stream.
//! - Each frame is prefixed with a serialized `ScreenFrameHeader` (length-prefixed postcard).
//! - The receiver drops any in-progress stream when a new keyframe stream arrives,
//!   ensuring the viewer always shows the latest decodable group.
//!
//! Priority: screen streams are lowest priority (3). Control and input streams
//! take precedence under congestion.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use bytes::Bytes;
use pairwire_core::{Error, Result};
use pairwire_protocol::ScreenFrameHeader;

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::connection::{PairwireConnection, send_message};

/// Sends encoded H.264 packets over QUIC streams to a remote peer.
///
/// Opens a new stream per keyframe group. Runs as a tokio task.
pub struct ScreenSender {
    conn: Arc<PairwireConnection>,
    stop: Arc<AtomicBool>,
}

impl ScreenSender {
    #[must_use]
    pub fn new(conn: Arc<PairwireConnection>) -> Self {
        Self {
            conn,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Spawn the sender task.
    ///
    /// Reads `EncodedPacket` from `packet_rx` and writes them to QUIC streams.
    /// Returns a handle that stops the task when dropped.
    #[must_use]
    pub fn spawn(
        self,
        mut packet_rx: mpsc::Receiver<pairwire_capture::EncodedPacket>,
    ) -> SenderHandle {
        let stop = Arc::clone(&self.stop);
        let stop_task = Arc::clone(&stop);
        let conn = Arc::clone(&self.conn);

        let handle = tokio::spawn(async move {
            let stop = stop_task;
            let mut current_stream: Option<iroh::endpoint::SendStream> = None;

            info!("screen sender started");

            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }

                let Some(pkt) = packet_rx.recv().await else {
                    break;
                };

                // Keepalives carry no video data — skip without opening a stream.
                if pkt.is_keepalive {
                    debug!("keepalive, no stream action");
                    continue;
                }

                // Keyframe: close current stream, open a new one
                if pkt.is_keyframe || current_stream.is_none() {
                    if let Some(mut s) = current_stream.take() {
                        let _ = s.finish();
                    }

                    match conn.open_screen_stream().await {
                        Ok(s) => {
                            debug!("opened new screen stream for keyframe");
                            current_stream = Some(s);
                        }
                        Err(e) => {
                            warn!(error = %e, "failed to open screen stream");
                            break;
                        }
                    }
                }

                let Some(stream) = current_stream.as_mut() else {
                    continue;
                };

                let header = if pkt.is_keyframe {
                    ScreenFrameHeader::new_keyframe(pkt.timestamp_us)
                } else {
                    ScreenFrameHeader::new_delta(pkt.timestamp_us)
                };

                // Write header then payload, length-prefixed
                if let Err(e) = send_message(stream, &header).await {
                    warn!(error = %e, "failed to write frame header");
                    current_stream = None;
                    continue;
                }

                // Write raw NAL data length-prefixed
                #[allow(clippy::cast_possible_truncation)] // NAL packets are always < 4GB
                let len = pkt.data.len() as u32;
                let mut len_buf = [0u8; 4];
                len_buf.copy_from_slice(&len.to_le_bytes());
                if stream.write_all(&len_buf).await.is_err()
                    || stream.write_all(&pkt.data).await.is_err()
                {
                    warn!("failed to write frame data");
                    current_stream = None;
                }
            }

            // Close the final stream cleanly
            if let Some(mut s) = current_stream {
                let _ = s.finish();
            }

            info!("screen sender stopped");
        });

        SenderHandle { stop, handle }
    }
}

/// Handle to a running `ScreenSender` task.
pub struct SenderHandle {
    stop: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

impl SenderHandle {
    pub async fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.await;
    }
}

/// Receives screen frames from a remote peer over QUIC streams.
///
/// Accepts incoming unidirectional streams. Each new stream is a new keyframe group.
/// When a new stream arrives, the previous one is abandoned (drop-old-groups policy).
pub struct ScreenReceiver {
    conn: Arc<PairwireConnection>,
    stop: Arc<AtomicBool>,
}

impl ScreenReceiver {
    #[must_use]
    pub fn new(conn: Arc<PairwireConnection>) -> Self {
        Self {
            conn,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Spawn the receiver task.
    ///
    /// Sends `EncodedPacket` to `packet_tx` for the decode thread.
    #[must_use]
    pub fn spawn(
        self,
        packet_tx: std::sync::mpsc::SyncSender<pairwire_capture::EncodedPacket>,
    ) -> ReceiverHandle {
        let stop = Arc::clone(&self.stop);
        let stop_task = Arc::clone(&stop);
        let conn = Arc::clone(&self.conn);

        let handle = tokio::spawn(async move {
            let stop = stop_task;
            info!("screen receiver started");

            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }

                // Accept the next incoming stream (each = new keyframe group)
                let stream = match conn.accept_screen_stream().await {
                    Ok(Some(s)) => s,
                    Ok(None) => break,
                    Err(e) => {
                        warn!(error = %e, "accept_screen_stream error");
                        break;
                    }
                };

                debug!("accepted new screen stream");

                // Drain this stream in a separate task so we can accept the next one
                let tx = packet_tx.clone();
                let stop2 = Arc::clone(&stop);
                tokio::spawn(async move {
                    if let Err(e) = drain_stream(stream, tx, stop2).await {
                        debug!(error = %e, "screen stream ended");
                    }
                });
            }

            info!("screen receiver stopped");
        });

        ReceiverHandle { stop, handle }
    }
}

async fn drain_stream(
    mut stream: iroh::endpoint::RecvStream,
    tx: std::sync::mpsc::SyncSender<pairwire_capture::EncodedPacket>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    use crate::connection::recv_message;
    use pairwire_capture::EncodedPacket;

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        // Read header
        let header: ScreenFrameHeader = match recv_message(&mut stream).await {
            Ok(h) => h,
            Err(Error::Transport(_)) => break, // stream closed
            Err(e) => return Err(e),
        };

        // Read NAL data (4-byte LE length prefix)
        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .await
            .map_err(|e| Error::Transport(format!("read nal len: {e}")))?;
        let len = u32::from_le_bytes(len_buf) as usize;

        if len > 4 * 1024 * 1024 {
            return Err(Error::Protocol(format!("NAL too large: {len} bytes")));
        }

        let mut data = vec![0u8; len];
        stream
            .read_exact(&mut data)
            .await
            .map_err(|e| Error::Transport(format!("read nal data: {e}")))?;

        let pkt = EncodedPacket {
            data: Bytes::from(data),
            timestamp_us: header.timestamp_us,
            is_keyframe: header.flags.keyframe,
            is_keepalive: false,
        };

        // try_send: drop if decode thread is behind (prefer latest)
        if tx.try_send(pkt).is_err() {
            debug!("decode channel full, dropping frame");
        }
    }

    Ok(())
}

/// Handle to a running `ScreenReceiver` task.
pub struct ReceiverHandle {
    stop: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

impl ReceiverHandle {
    pub async fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.await;
    }
}
