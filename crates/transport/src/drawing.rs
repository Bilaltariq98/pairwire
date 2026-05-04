//! Drawing and annotation transport.
//!
//! Two-tier data flow:
//! - `DrawPoint` datagrams: sent while the user is drawing. Lossy is fine — worst
//!   case a point is skipped in the live preview. Tagged with `TAG_DRAW_POINT`.
//! - `DrawStroke` / `DrawClear` stream messages: sent on a dedicated bidi annotation
//!   stream opened before moq-lite starts (same pattern as the input stream). This
//!   avoids conflicting with moq-lite's `accept_uni()` loop.
//!
//! Stream direction: guest opens the annotation bidi stream; host accepts it.
//! Both peers write `ControlMessage` frames to their send half and read from their
//! recv half — the stream is fully bidirectional.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use bytes::Bytes;
use iroh::endpoint::{RecvStream, SendStream};
use pairwire_core::{Error, Result};
use pairwire_protocol::ControlMessage;
use pairwire_protocol::drawing::{DrawClear, DrawPoint, DrawStroke};
use postcard::to_allocvec;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use crate::connection::PairwireConnection;
use crate::connection::{recv_message, send_message};
use crate::datagram_router::{self, TAG_DRAW_POINT};

// ── Sender ────────────────────────────────────────────────────────────────────

/// Events the viewer sends to `DrawingSender`.
#[derive(Clone)]
pub enum DrawingEvent {
    /// A point in the current stroke — sent as a datagram for live preview.
    Point(DrawPoint),
    /// A completed stroke — sent on the annotation stream.
    Stroke(DrawStroke),
    /// Clear strokes — sent on the annotation stream.
    Clear(DrawClear),
}

/// Sends drawing events to the remote peer.
///
/// `DrawPoint` events go as tagged datagrams (unreliable, low-latency).
/// `DrawStroke` and `DrawClear` go as `ControlMessage` frames on the annotation stream.
pub struct DrawingSender {
    conn: Arc<PairwireConnection>,
    stream: SendStream,
}

impl DrawingSender {
    #[must_use]
    pub fn new(conn: Arc<PairwireConnection>, stream: SendStream) -> Self {
        Self { conn, stream }
    }

    /// Spawn the sender task.
    #[must_use]
    pub fn spawn(self, mut event_rx: mpsc::Receiver<DrawingEvent>) -> DrawingSenderHandle {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_task = Arc::clone(&stop);
        let conn = self.conn;
        let mut stream = self.stream;

        let handle = tokio::spawn(async move {
            info!("drawing sender started");
            while !stop_task.load(Ordering::Relaxed) {
                let Some(event) = event_rx.recv().await else {
                    break;
                };
                match event {
                    DrawingEvent::Point(pt) => match to_allocvec(&pt) {
                        Ok(payload) => {
                            if let Err(e) = datagram_router::send_tagged(
                                conn.raw_conn(),
                                TAG_DRAW_POINT,
                                &payload,
                            ) {
                                warn!(error = %e, "draw point datagram send failed");
                            } else {
                                debug!(x = pt.x, y = pt.y, "draw point sent");
                            }
                        }
                        Err(e) => warn!(error = %e, "draw point serialization failed"),
                    },
                    DrawingEvent::Stroke(stroke) => {
                        let msg = ControlMessage::DrawStroke(stroke);
                        if let Err(e) = send_message(&mut stream, &msg).await {
                            warn!(error = %e, "draw stroke send failed");
                            break;
                        }
                        debug!("draw stroke sent");
                    }
                    DrawingEvent::Clear(clear) => {
                        let msg = ControlMessage::DrawClear(clear);
                        if let Err(e) = send_message(&mut stream, &msg).await {
                            warn!(error = %e, "draw clear send failed");
                            break;
                        }
                        debug!("draw clear sent");
                    }
                }
            }
            let _ = stream.finish();
            info!("drawing sender stopped");
        });

        DrawingSenderHandle { stop, handle }
    }
}

pub struct DrawingSenderHandle {
    stop: Arc<AtomicBool>,
    handle: tokio::task::JoinHandle<()>,
}

impl DrawingSenderHandle {
    pub async fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.await;
    }
}

// ── Receiver ──────────────────────────────────────────────────────────────────

/// Maximum strokes retained per peer. Older strokes are evicted FIFO.
/// Prevents unbounded memory growth in long sessions with persistent ink.
pub const STROKES_PER_PEER_CAP: usize = 200;

/// A stroke tagged with the local time it was received.
/// Used by the viewer to time-fade laser strokes.
#[derive(Clone, Debug)]
pub struct TimedStroke {
    pub stroke: DrawStroke,
    pub received_at: Instant,
}

/// Receives drawing events from the remote peer.
///
/// Publishes accumulated strokes via `watch::Receiver<Vec<TimedStroke>>` and
/// live draw points via `watch::Receiver<Option<DrawPoint>>`.
pub struct DrawingReceiver {
    stream: RecvStream,
}

impl DrawingReceiver {
    #[must_use]
    pub fn new(stream: RecvStream) -> Self {
        Self { stream }
    }

    /// Spawn the receiver task.
    ///
    /// - `point_rx`: routed `DrawPoint` datagram payloads from `DatagramRouter`
    ///
    /// Returns a handle, accumulated strokes watch, and live point watch.
    #[must_use]
    pub fn spawn(
        self,
        mut point_rx: mpsc::Receiver<Bytes>,
    ) -> (
        DrawingReceiverHandle,
        watch::Receiver<Vec<TimedStroke>>,
        watch::Receiver<Option<DrawPoint>>,
    ) {
        let (strokes_tx, strokes_rx) = watch::channel(Vec::new());
        let (live_tx, live_rx) = watch::channel(None);
        let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
        let mut stream = self.stream;

        let handle = tokio::spawn(async move {
            info!("drawing receiver started");
            let mut strokes: Vec<TimedStroke> = Vec::new();

            loop {
                tokio::select! {
                    biased;
                    _ = &mut stop_rx => break,

                    // Completed stroke or clear from the annotation stream.
                    result = recv_annotation(&mut stream) => {
                        match result {
                            Err(e) => {
                                let s = e.to_string();
                                if s.contains("closed") || s.contains("reset") || s.contains("eof") {
                                    break;
                                }
                                warn!(error = %s, "annotation stream recv error");
                                break;
                            }
                            Ok(msg) => match msg {
                                ControlMessage::DrawStroke(stroke) => {
                                    debug!(
                                        points = stroke.points.len(),
                                        mode = ?stroke.mode,
                                        peer = stroke.peer_id,
                                        "draw stroke received"
                                    );
                                    let peer = stroke.peer_id;
                                    strokes.push(TimedStroke {
                                        stroke,
                                        received_at: Instant::now(),
                                    });
                                    // Per-peer FIFO cap: evict oldest from this peer
                                    // until we're at or below the cap.
                                    let count = strokes.iter().filter(|t| t.stroke.peer_id == peer).count();
                                    if count > STROKES_PER_PEER_CAP {
                                        let to_remove = count - STROKES_PER_PEER_CAP;
                                        let mut removed = 0;
                                        strokes.retain(|t| {
                                            if removed < to_remove && t.stroke.peer_id == peer {
                                                removed += 1;
                                                false
                                            } else {
                                                true
                                            }
                                        });
                                    }
                                    let _ = strokes_tx.send(strokes.clone());
                                }
                                ControlMessage::DrawClear(clear) => {
                                    debug!(peer = ?clear.peer_id, "draw clear received");
                                    match clear.peer_id {
                                        Some(id) => strokes.retain(|t| t.stroke.peer_id != id),
                                        None => strokes.clear(),
                                    }
                                    let _ = strokes_tx.send(strokes.clone());
                                }
                                other => {
                                    debug!(msg = ?other, "unexpected message on annotation stream");
                                }
                            }
                        }
                    }

                    // Live draw point datagram — transient preview only.
                    maybe_payload = point_rx.recv() => {
                        let Some(payload) = maybe_payload else { break };
                        match postcard::from_bytes::<DrawPoint>(&payload) {
                            Ok(pt) => {
                                debug!(x = pt.x, y = pt.y, peer = pt.peer_id, "draw point received");
                                let _ = live_tx.send(Some(pt));
                            }
                            Err(e) => debug!(error = %e, "invalid draw point datagram"),
                        }
                    }
                }
            }

            info!("drawing receiver stopped");
        });

        (
            DrawingReceiverHandle { handle, stop_tx },
            strokes_rx,
            live_rx,
        )
    }
}

async fn recv_annotation(stream: &mut RecvStream) -> Result<ControlMessage> {
    recv_message(stream)
        .await
        .map_err(|e| Error::Transport(format!("annotation recv: {e}")))
}

pub struct DrawingReceiverHandle {
    handle: tokio::task::JoinHandle<()>,
    stop_tx: oneshot::Sender<()>,
}

impl DrawingReceiverHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.handle.await;
    }
}

#[cfg(test)]
mod tests {
    use pairwire_protocol::drawing::{DrawMode, DrawPoint, DrawStroke};
    use postcard::to_allocvec;

    #[test]
    fn draw_point_serializes_under_20_bytes() {
        let pt = DrawPoint {
            peer_id: u8::MAX,
            x: u16::MAX,
            y: u16::MAX,
        };
        let bytes = to_allocvec(&pt).unwrap();
        assert!(
            bytes.len() < 20,
            "DrawPoint serialized to {} bytes, must be < 20 for datagram budget",
            bytes.len()
        );
    }

    #[test]
    fn draw_stroke_round_trip() {
        let stroke = DrawStroke {
            peer_id: 1,
            color: [255, 128, 0],
            points: vec![(100, 200), (150, 250)],
            mode: DrawMode::Laser,
        };
        let bytes = to_allocvec(&stroke).unwrap();
        let decoded: DrawStroke = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(stroke, decoded);
    }
}
