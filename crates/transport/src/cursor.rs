//! Cursor presence sharing via QUIC datagrams.
//!
//! Sent unreliably at up to 60Hz. Dropped datagrams are acceptable —
//! the viewer always shows the latest received position.
//!
//! The `CursorSender` runs a tokio task that reads from an `mpsc` channel
//! and fires datagrams. The `CursorReceiver` runs a tokio task that reads
//! datagrams and writes the latest position to an `ArcSwap`-style atomic slot.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use pairwire_core::Error;
use pairwire_protocol::CursorUpdate;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use crate::connection::PairwireConnection;

/// Sends cursor updates as QUIC datagrams at up to 60Hz.
pub struct CursorSender {
    conn: Arc<PairwireConnection>,
}

impl CursorSender {
    #[must_use]
    pub fn new(conn: Arc<PairwireConnection>) -> Self {
        Self { conn }
    }

    /// Spawn the sender task.
    ///
    /// Reads `CursorUpdate` from `update_rx` and sends each as a datagram.
    /// The channel should be bounded to 1 — only the latest position matters.
    #[must_use]
    pub fn spawn(self, mut update_rx: mpsc::Receiver<CursorUpdate>) -> CursorSenderHandle {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_task = Arc::clone(&stop);
        let conn = self.conn;

        let handle = tokio::spawn(async move {
            info!("cursor sender started");
            while !stop_task.load(Ordering::Relaxed) {
                let Some(update) = update_rx.recv().await else {
                    break;
                };
                if let Err(e) = conn.send_cursor_datagram(&update) {
                    warn!(error = %e, "cursor datagram send failed");
                }
                debug!(x = update.x, y = update.y, "cursor sent");
            }
            info!("cursor sender stopped");
        });

        CursorSenderHandle { stop, handle }
    }
}

pub struct CursorSenderHandle {
    stop: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

impl CursorSenderHandle {
    pub async fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.await;
    }
}

/// Receives cursor updates from QUIC datagrams.
///
/// Exposes the latest cursor position via a `watch::Receiver<Option<CursorUpdate>>`.
/// Consumers subscribe to the watch channel and re-render on change.
pub struct CursorReceiver {
    conn: Arc<PairwireConnection>,
}

impl CursorReceiver {
    #[must_use]
    pub fn new(conn: Arc<PairwireConnection>) -> Self {
        Self { conn }
    }

    /// Spawn the receiver task.
    ///
    /// Returns a `watch::Receiver` that always holds the latest cursor state.
    #[must_use]
    pub fn spawn(self) -> (CursorReceiverHandle, watch::Receiver<Option<CursorUpdate>>) {
        let (tx, rx) = watch::channel(None);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_task = Arc::clone(&stop);
        let conn = self.conn;

        let handle = tokio::spawn(async move {
            info!("cursor receiver started");
            while !stop_task.load(Ordering::Relaxed) {
                match conn.recv_cursor_datagram().await {
                    Ok(update) => {
                        debug!(x = update.x, y = update.y, "cursor received");
                        let _ = tx.send(Some(update));
                    }
                    Err(Error::Transport(_)) => break,
                    Err(e) => {
                        warn!(error = %e, "cursor datagram recv error");
                        break;
                    }
                }
            }
            info!("cursor receiver stopped");
        });

        (CursorReceiverHandle { stop, handle }, rx)
    }
}

pub struct CursorReceiverHandle {
    stop: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

impl CursorReceiverHandle {
    pub async fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.await;
    }
}

#[cfg(test)]
mod tests {
    use pairwire_protocol::CursorUpdate;

    #[test]
    fn cursor_update_serializes_under_mtu() {
        use postcard::to_allocvec;
        let update = CursorUpdate {
            peer_id: 1,
            x: 1920,
            y: 1080,
            visible: true,
            click_animation: 0,
        };
        let bytes = to_allocvec(&update).unwrap();
        assert!(
            bytes.len() < 1200,
            "cursor datagram {} bytes exceeds QUIC MTU budget",
            bytes.len()
        );
    }
}
