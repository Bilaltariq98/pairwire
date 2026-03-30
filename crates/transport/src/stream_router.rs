//! Uni-directional stream router.
//!
//! All incoming uni streams share the same QUIC `accept_uni()` call.
//! The router reads a 1-byte type tag from each new stream and forwards it
//! to the appropriate consumer channel:
//!
//! - `SCREEN_STREAM_TAG` (0x53 / 'S') → screen receiver
//! - Anything else → moq-lite (with the consumed byte prepended back)
//!
//! This prevents moq-lite from accidentally consuming screen streams
//! and vice versa.

use iroh::endpoint::{Connection, RecvStream};
use tokio::sync::mpsc;
use tracing::debug;

/// First byte written to every screen uni stream opened by `ScreenSender`.
pub const SCREEN_STREAM_TAG: u8 = 0x53; // ASCII 'S'

/// A non-screen stream with its consumed first byte preserved.
pub struct TaggedStream {
    pub tag: u8,
    pub stream: RecvStream,
}

/// Routes incoming unidirectional QUIC streams by a 1-byte type tag.
pub struct UniStreamRouter {
    screen_rx: mpsc::Receiver<RecvStream>,
    _handle: tokio::task::JoinHandle<()>,
}

impl UniStreamRouter {
    /// Start the router. Returns the router (for screen streams) and a
    /// `SharedNonScreenRouter` that `IrohWtSession` uses for `accept_uni`.
    pub fn start(conn: Connection) -> (Self, NonScreenReceiver) {
        let (screen_tx, screen_rx) = mpsc::channel::<RecvStream>(8);
        let (other_tx, other_rx) = mpsc::channel::<TaggedStream>(8);

        let handle = tokio::spawn(async move {
            loop {
                let mut stream = match conn.accept_uni().await {
                    Ok(s) => s,
                    Err(_) => break,
                };

                let mut tag = [0u8; 1];
                if stream.read_exact(&mut tag).await.is_err() {
                    debug!("uni stream closed before type tag");
                    continue;
                }

                if tag[0] == SCREEN_STREAM_TAG {
                    if screen_tx.send(stream).await.is_err() {
                        break;
                    }
                } else {
                    let tagged = TaggedStream {
                        tag: tag[0],
                        stream,
                    };
                    if other_tx.send(tagged).await.is_err() {
                        break;
                    }
                }
            }
            debug!("uni stream router stopped");
        });

        let non_screen = NonScreenReceiver {
            rx: std::sync::Arc::new(tokio::sync::Mutex::new(other_rx)),
        };

        (
            Self {
                screen_rx,
                _handle: handle,
            },
            non_screen,
        )
    }

    /// Accept the next screen stream (tag already consumed).
    pub async fn accept_screen(&mut self) -> Option<RecvStream> {
        self.screen_rx.recv().await
    }
}

/// Shared receiver for non-screen uni streams. Clone-safe for `IrohWtSession`.
#[derive(Clone)]
pub struct NonScreenReceiver {
    rx: std::sync::Arc<tokio::sync::Mutex<mpsc::Receiver<TaggedStream>>>,
}

impl NonScreenReceiver {
    /// Accept the next non-screen uni stream with its consumed tag byte.
    pub async fn recv(&self) -> Option<TaggedStream> {
        self.rx.lock().await.recv().await
    }
}
