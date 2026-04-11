//! Cursor presence sharing via tagged QUIC datagrams.
//!
//! Sent unreliably at up to 60Hz. Dropped datagrams are acceptable —
//! the viewer always shows the latest received position.
//!
//! The `CursorSender` runs a tokio task that reads from an `mpsc` channel
//! and fires tagged datagrams. The `CursorReceiver` runs a tokio task that
//! reads payloads from the datagram router and publishes the latest position.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use bytes::Bytes;
use pairwire_core::Error;
use pairwire_protocol::CursorUpdate;
use postcard::to_allocvec;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use crate::connection::PairwireConnection;
use crate::datagram_router::{self, TAG_CURSOR};

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
    /// Reads `CursorUpdate` from `update_rx` and sends each as a tagged datagram.
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
                let result = to_allocvec(&update)
                    .map_err(Error::from)
                    .and_then(|payload| {
                        datagram_router::send_tagged(conn.raw_conn(), TAG_CURSOR, &payload)
                            .map_err(Error::Transport)
                    });

                if let Err(e) = result {
                    warn!(error = %e, "cursor datagram send failed");
                } else {
                    debug!(x = update.x, y = update.y, "cursor sent");
                }
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

/// Receives cursor updates from routed datagram payloads.
///
/// Exposes the latest cursor position via a `watch::Receiver<Option<CursorUpdate>>`.
/// Consumers subscribe to the watch channel and re-render on change.
pub struct CursorReceiver;

impl CursorReceiver {
    /// Spawn the receiver task.
    ///
    /// Returns a `watch::Receiver` that always holds the latest cursor state.
    #[must_use]
    pub fn spawn(
        mut cursor_rx: mpsc::Receiver<Bytes>,
    ) -> (CursorReceiverHandle, watch::Receiver<Option<CursorUpdate>>) {
        let (tx, rx) = watch::channel(None);

        let handle = tokio::spawn(async move {
            info!("cursor receiver started");
            while let Some(payload) = cursor_rx.recv().await {
                match postcard::from_bytes::<CursorUpdate>(&payload) {
                    Ok(update) => {
                        debug!(x = update.x, y = update.y, "cursor received");
                        let _ = tx.send(Some(update));
                    }
                    Err(e) => {
                        debug!(error = %e, "invalid cursor datagram");
                    }
                }
            }
            info!("cursor receiver stopped");
        });

        (CursorReceiverHandle { handle }, rx)
    }
}

pub struct CursorReceiverHandle {
    handle: tokio::task::JoinHandle<()>,
}

impl CursorReceiverHandle {
    pub async fn stop(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use pairwire_protocol::CursorUpdate;
    use tokio::sync::mpsc;

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

    #[tokio::test]
    async fn cursor_receiver_publishes_latest_update() {
        let (tx, rx) = mpsc::channel(4);
        let (handle, mut watch_rx) = super::CursorReceiver::spawn(rx);
        let update = CursorUpdate {
            peer_id: 7,
            x: 640,
            y: 480,
            visible: true,
            click_animation: 2,
        };

        tx.send(Bytes::from(postcard::to_allocvec(&update).unwrap()))
            .await
            .unwrap();
        drop(tx);

        watch_rx.changed().await.unwrap();
        assert_eq!(watch_rx.borrow().as_ref(), Some(&update));

        handle.stop().await;
    }

    #[tokio::test]
    async fn cursor_receiver_ignores_invalid_payload() {
        let (tx, rx) = mpsc::channel(4);
        let (handle, mut watch_rx) = super::CursorReceiver::spawn(rx);

        tx.send(Bytes::from_static(&[1, 2, 3])).await.unwrap();
        drop(tx);

        assert!(watch_rx.changed().await.is_err());
        assert!(watch_rx.borrow().is_none());

        handle.stop().await;
    }
}
