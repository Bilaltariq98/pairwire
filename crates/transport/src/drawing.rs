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
//!
//! # Stroke cache and replay
//!
//! The host maintains a long-lived `StrokeCache` (created once at session startup,
//! outliving individual guest connections). Only `DrawMode::Persistent` strokes are
//! cached; `Laser` strokes are ephemeral and never replayed.
//!
//! When a new guest annotation stream is accepted the host calls
//! `replay_persistent_strokes` to flush all cached strokes back to the guest so it
//! sees the existing whiteboard immediately.
//!
//! ## Reconnect lifecycle note
//!
//! The host iroh `Endpoint` stays alive after a guest disconnects — `run_host`
//! currently calls `transport.accept()` only once, so it does **not** loop to
//! accept a second connection. A future host-reconnect loop would:
//! 1. Call `transport.accept()` again after `conn.closed().await`.
//! 2. Redo handshake + stream setup with the new conn.
//! 3. Call `replay_persistent_strokes(cache, sender_tx)` right after accepting the
//!    annotation stream — that is the **exact call site** documented here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bytes::Bytes;
use iroh::endpoint::{RecvStream, SendStream};
use pairwire_core::{Error, Result};
use pairwire_protocol::ControlMessage;
use pairwire_protocol::drawing::{DrawClear, DrawMode, DrawPoint, DrawStroke};
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

// ── Stroke Cache ──────────────────────────────────────────────────────────────

/// Maximum strokes retained per peer. Older strokes are evicted FIFO.
/// Prevents unbounded memory growth in long sessions with persistent ink.
pub const STROKES_PER_PEER_CAP: usize = 200;

/// Long-lived, session-scoped store of persistent strokes on the host.
///
/// Created once when `run_host` starts and shared (via `Arc`) across
/// successive guest connections. Only `DrawMode::Persistent` strokes
/// are inserted; `Laser` strokes are intentionally excluded because they
/// are ephemeral gestures that would be meaningless on replay.
///
/// `DrawClear` commands are applied immediately so the cache always
/// reflects the whiteboard state a reconnecting guest should see.
///
/// Thread-safe via `Mutex` — all access is through a shared `Arc<StrokeCache>`.
#[derive(Debug, Default)]
pub struct StrokeCache {
    inner: Mutex<Vec<DrawStroke>>,
}

impl StrokeCache {
    /// Create an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a persistent stroke. Laser strokes are silently dropped.
    ///
    /// Applies the per-peer FIFO cap (`STROKES_PER_PEER_CAP`) after insertion.
    pub fn insert(&self, stroke: DrawStroke) {
        if stroke.mode != DrawMode::Persistent {
            return;
        }
        let mut strokes = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let peer = stroke.peer_id;
        strokes.push(stroke);

        // Per-peer FIFO cap: evict oldest from this peer until at or below the cap.
        let count = strokes.iter().filter(|s| s.peer_id == peer).count();
        if count > STROKES_PER_PEER_CAP {
            let to_remove = count - STROKES_PER_PEER_CAP;
            let mut removed = 0usize;
            strokes.retain(|s| {
                if removed < to_remove && s.peer_id == peer {
                    removed += 1;
                    false
                } else {
                    true
                }
            });
        }
    }

    /// Apply a `DrawClear` — mirrors the receiver's clear logic.
    pub fn apply_clear(&self, clear: &DrawClear) {
        let mut strokes = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match clear.peer_id {
            Some(id) => strokes.retain(|s| s.peer_id != id),
            None => strokes.clear(),
        }
    }

    /// Return a snapshot of all cached strokes.
    #[must_use]
    pub fn snapshot(&self) -> Vec<DrawStroke> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Number of cached strokes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// True if there are no cached strokes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Replay all cached persistent strokes to a freshly connected guest.
///
/// Sends each stroke as a `DrawingEvent::Stroke` to the provided channel.
/// Respects the per-peer cap already enforced inside `StrokeCache::insert`.
///
/// # Call site for future host-reconnect work
///
/// When the host accept loop is extended to handle multiple successive guests
/// (see `run_host` in `crates/app/src/session.rs`), call this function
/// immediately after accepting the annotation bidi stream and before the
/// `DrawingSender::spawn` invocation, passing the same `draw_event_tx` that
/// was created for the new stream:
///
/// ```text
/// replay_persistent_strokes(&cache, &draw_event_tx).await;
/// host_drawing_send_handle = Some(sender.spawn(draw_event_rx));
/// ```
pub async fn replay_persistent_strokes(
    cache: &StrokeCache,
    draw_event_tx: &mpsc::Sender<DrawingEvent>,
) {
    let strokes = cache.snapshot();
    let count = strokes.len();
    if count == 0 {
        debug!("stroke replay: cache empty, nothing to send");
        return;
    }
    info!(count, "replaying persistent strokes to new guest");
    for stroke in strokes {
        if draw_event_tx
            .send(DrawingEvent::Stroke(stroke))
            .await
            .is_err()
        {
            warn!("stroke replay: drawing sender closed before replay complete");
            break;
        }
    }
    debug!("stroke replay complete");
}

// ── Receiver ──────────────────────────────────────────────────────────────────

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
        point_rx: mpsc::Receiver<Bytes>,
    ) -> (
        DrawingReceiverHandle,
        watch::Receiver<Vec<TimedStroke>>,
        watch::Receiver<Option<DrawPoint>>,
    ) {
        self.spawn_inner(point_rx, None)
    }

    /// Spawn the receiver task with a shared `StrokeCache`.
    ///
    /// Persistent strokes are inserted into `cache` on arrival and cleared from
    /// it when a `DrawClear` is received. The cache is designed to survive
    /// across guest disconnects so strokes can be replayed on reconnect.
    #[must_use]
    pub fn spawn_with_cache(
        self,
        point_rx: mpsc::Receiver<Bytes>,
        cache: Arc<StrokeCache>,
    ) -> (
        DrawingReceiverHandle,
        watch::Receiver<Vec<TimedStroke>>,
        watch::Receiver<Option<DrawPoint>>,
    ) {
        self.spawn_inner(point_rx, Some(cache))
    }

    fn spawn_inner(
        self,
        mut point_rx: mpsc::Receiver<Bytes>,
        cache: Option<Arc<StrokeCache>>,
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
                                    // Update the long-lived cache before the local strokes Vec
                                    // so both stay consistent.
                                    if let Some(ref c) = cache {
                                        c.insert(stroke.clone());
                                    }
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
                                    if let Some(ref c) = cache {
                                        c.apply_clear(&clear);
                                    }
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
    use std::sync::Arc;

    use pairwire_protocol::drawing::{DrawClear, DrawMode, DrawPoint, DrawStroke};
    use postcard::to_allocvec;

    use super::{DrawingEvent, STROKES_PER_PEER_CAP, StrokeCache, replay_persistent_strokes};

    fn make_stroke(peer_id: u8, mode: DrawMode) -> DrawStroke {
        DrawStroke {
            peer_id,
            color: [255, 0, 0],
            points: vec![(10, 20), (30, 40)],
            mode,
        }
    }

    // ── StrokeCache unit tests ─────────────────────────────────────────────────

    #[test]
    fn cache_inserts_persistent_strokes_only() {
        let cache = StrokeCache::new();
        cache.insert(make_stroke(1, DrawMode::Persistent));
        cache.insert(make_stroke(1, DrawMode::Laser)); // must be ignored
        assert_eq!(cache.len(), 1);
        let snap = cache.snapshot();
        assert_eq!(snap[0].mode, DrawMode::Persistent);
    }

    #[test]
    fn cache_evicts_oldest_when_per_peer_cap_exceeded() {
        let cache = StrokeCache::new();
        // Insert cap+1 persistent strokes for peer 0.
        for _ in 0..=STROKES_PER_PEER_CAP {
            cache.insert(make_stroke(0, DrawMode::Persistent));
        }
        // Exactly cap strokes must remain.
        assert_eq!(cache.len(), STROKES_PER_PEER_CAP);
    }

    #[test]
    fn cache_cap_is_per_peer_not_global() {
        let cache = StrokeCache::new();
        // Fill peer 0 to exactly the cap.
        for _ in 0..STROKES_PER_PEER_CAP {
            cache.insert(make_stroke(0, DrawMode::Persistent));
        }
        // Add one stroke for a different peer — total may exceed cap globally.
        cache.insert(make_stroke(1, DrawMode::Persistent));
        assert_eq!(cache.len(), STROKES_PER_PEER_CAP + 1);
    }

    #[test]
    fn clear_all_empties_cache() {
        let cache = StrokeCache::new();
        cache.insert(make_stroke(0, DrawMode::Persistent));
        cache.insert(make_stroke(1, DrawMode::Persistent));
        cache.apply_clear(&DrawClear { peer_id: None });
        assert!(cache.is_empty());
    }

    #[test]
    fn clear_peer_removes_only_that_peer() {
        let cache = StrokeCache::new();
        cache.insert(make_stroke(0, DrawMode::Persistent));
        cache.insert(make_stroke(1, DrawMode::Persistent));
        cache.apply_clear(&DrawClear { peer_id: Some(0) });
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.snapshot()[0].peer_id, 1);
    }

    #[test]
    fn snapshot_returns_all_strokes_immutably() {
        let cache = StrokeCache::new();
        cache.insert(make_stroke(0, DrawMode::Persistent));
        let snap1 = cache.snapshot();
        cache.insert(make_stroke(0, DrawMode::Persistent));
        let snap2 = cache.snapshot();
        // snap1 must not be affected by the second insert.
        assert_eq!(snap1.len(), 1);
        assert_eq!(snap2.len(), 2);
    }

    // ── replay_persistent_strokes tests ───────────────────────────────────────

    #[tokio::test]
    async fn replay_sends_all_cached_strokes_as_events() {
        let cache = StrokeCache::new();
        cache.insert(make_stroke(0, DrawMode::Persistent));
        cache.insert(make_stroke(1, DrawMode::Persistent));

        let (tx, mut rx) = tokio::sync::mpsc::channel::<DrawingEvent>(16);
        replay_persistent_strokes(&cache, &tx).await;
        drop(tx);

        let mut received = Vec::new();
        while let Some(event) = rx.recv().await {
            if let DrawingEvent::Stroke(s) = event {
                received.push(s);
            }
        }
        assert_eq!(received.len(), 2);
    }

    #[tokio::test]
    async fn replay_sends_nothing_when_cache_empty() {
        let cache = StrokeCache::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<DrawingEvent>(16);
        replay_persistent_strokes(&cache, &tx).await;
        drop(tx);
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn replay_stops_gracefully_when_receiver_dropped() {
        let cache = StrokeCache::new();
        for _ in 0..10 {
            cache.insert(make_stroke(0, DrawMode::Persistent));
        }
        // Drop the receiver immediately — replay must not panic or deadlock.
        let (tx, rx) = tokio::sync::mpsc::channel::<DrawingEvent>(1);
        drop(rx);
        // Should return without hanging.
        replay_persistent_strokes(&cache, &tx).await;
    }

    #[test]
    fn cache_is_arc_shareable() {
        // Verify StrokeCache can be shared across threads via Arc.
        let cache = Arc::new(StrokeCache::new());
        let cache2 = Arc::clone(&cache);
        std::thread::spawn(move || {
            cache2.insert(make_stroke(0, DrawMode::Persistent));
        })
        .join()
        .unwrap();
        assert_eq!(cache.len(), 1);
    }

    // ── Pre-existing serialisation tests ──────────────────────────────────────

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
