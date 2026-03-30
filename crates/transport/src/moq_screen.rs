//! moq-lite screen track: publish and subscribe to H.264 screen frames.
//!
//! The host publishes a BroadcastProducer with a single "screen" track (priority 3).
//! Each keyframe starts a new moq-lite Group. Delta frames are appended to the current group.
//! The guest subscribes and feeds received frames to the decode channel.
//!
//! Transport: `web_transport_iroh::Session::raw(conn)` wraps the iroh Connection as a
//! WebTransport session. moq-lite's Client/Server perform the MoQ SETUP handshake on top.
//!
//! This replaces the raw QUIC stream screen transport from Phase 1 (screen.rs).

use std::sync::Arc;

use moq_lite::{BroadcastProducer, Client, OriginProducer, Server, Track, TrackConsumer};
use pairwire_capture::EncodedPacket;
use pairwire_core::{Error, Result};
use tokio::{sync::oneshot, task::JoinHandle};
use tracing::{debug, info, instrument, warn};

use crate::{connection::PairwireConnection, wt_session::IrohWtSession};

/// Name of the screen track within the broadcast.
pub const SCREEN_TRACK_NAME: &str = "screen";

/// moq-lite track priority for screen frames (lower = higher priority; 3 = lowest).
pub const SCREEN_TRACK_PRIORITY: u8 = 3;

/// Path under which the screen broadcast is published.
pub const SCREEN_BROADCAST_PATH: &str = "pairwire/screen";

// ── Publisher (host side) ────────────────────────────────────────────────────

/// Handle for the moq-lite screen publisher task.
pub struct MoqScreenPublisherHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl MoqScreenPublisherHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

/// Publishes encoded screen frames over a moq-lite track.
///
/// Reads `EncodedPacket` from a channel. Each keyframe starts a new Group;
/// delta frames are appended as Frames within the current Group.
pub struct MoqScreenPublisher {
    conn: Arc<PairwireConnection>,
}

impl MoqScreenPublisher {
    pub fn new(conn: Arc<PairwireConnection>) -> Self {
        Self { conn }
    }

    #[instrument(skip(self, packet_rx))]
    pub fn spawn(
        self,
        packet_rx: tokio::sync::mpsc::Receiver<EncodedPacket>,
    ) -> MoqScreenPublisherHandle {
        let (stop_tx, stop_rx) = oneshot::channel();

        let join = tokio::spawn(async move {
            if let Err(e) = publish_loop(self.conn, packet_rx, stop_rx).await {
                warn!(error = %e, "moq screen publisher error");
            }
            info!("moq screen publisher stopped");
        });

        MoqScreenPublisherHandle { stop_tx, join }
    }
}

async fn publish_loop(
    conn: Arc<PairwireConnection>,
    mut packet_rx: tokio::sync::mpsc::Receiver<EncodedPacket>,
    mut stop_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let wt = IrohWtSession::new(conn.raw_conn().clone());

    // Build the origin with a broadcast containing the screen track.
    let origin = OriginProducer::new();
    let mut broadcast = BroadcastProducer::new();
    let mut track = broadcast
        .create_track(Track {
            name: SCREEN_TRACK_NAME.to_string(),
            priority: SCREEN_TRACK_PRIORITY,
        })
        .map_err(|e| Error::Transport(format!("moq create_track: {e}")))?;

    origin.publish_broadcast(SCREEN_BROADCAST_PATH, broadcast.consume());

    // MoQ SETUP handshake — host acts as server (publisher).
    let _session = Server::new()
        .with_publish(origin.consume())
        .accept(wt)
        .await
        .map_err(|e| Error::Transport(format!("moq server accept: {e}")))?;

    info!("moq screen publisher ready");

    let mut current_group: Option<moq_lite::GroupProducer> = None;

    loop {
        tokio::select! {
            biased;
            _ = &mut stop_rx => break,
            pkt = packet_rx.recv() => {
                let Some(pkt) = pkt else { break };

                // Start a new group on each keyframe.
                if pkt.is_keyframe || current_group.is_none() {
                    if let Some(mut g) = current_group.take() {
                        let _ = g.finish();
                    }
                    current_group = Some(
                        track
                            .append_group()
                            .map_err(|e| Error::Transport(format!("moq append_group: {e}")))?,
                    );
                    debug!("new moq group (keyframe)");
                }

                if let Some(group) = &mut current_group {
                    group
                        .write_frame(pkt.data)
                        .map_err(|e| Error::Transport(format!("moq write_frame: {e}")))?;
                }
            }
        }
    }

    if let Some(mut g) = current_group.take() {
        let _ = g.finish();
    }

    Ok(())
}

// ── Subscriber (guest side) ──────────────────────────────────────────────────

/// Handle for the moq-lite screen subscriber task.
pub struct MoqScreenSubscriberHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl MoqScreenSubscriberHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

/// Subscribes to the host's screen track and feeds packets to the decode channel.
pub struct MoqScreenSubscriber {
    conn: Arc<PairwireConnection>,
}

impl MoqScreenSubscriber {
    pub fn new(conn: Arc<PairwireConnection>) -> Self {
        Self { conn }
    }

    #[instrument(skip(self, pkt_tx))]
    pub fn spawn(
        self,
        pkt_tx: std::sync::mpsc::SyncSender<EncodedPacket>,
    ) -> MoqScreenSubscriberHandle {
        let (stop_tx, stop_rx) = oneshot::channel();

        let join = tokio::spawn(async move {
            if let Err(e) = subscribe_loop(self.conn, pkt_tx, stop_rx).await {
                warn!(error = %e, "moq screen subscriber error");
            }
            info!("moq screen subscriber stopped");
        });

        MoqScreenSubscriberHandle { stop_tx, join }
    }
}

async fn subscribe_loop(
    conn: Arc<PairwireConnection>,
    pkt_tx: std::sync::mpsc::SyncSender<EncodedPacket>,
    mut stop_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let wt = IrohWtSession::new(conn.raw_conn().clone());

    let origin = OriginProducer::new();

    // MoQ SETUP handshake — guest acts as client (subscriber).
    let _session = Client::new()
        .with_consume(origin.clone())
        .connect(wt)
        .await
        .map_err(|e| Error::Transport(format!("moq client connect: {e}")))?;

    info!("moq screen subscriber connected");

    // Wait for the screen broadcast to be announced.
    let mut origin_rx = origin.consume();
    let broadcast_consumer = tokio::select! {
        biased;
        _ = &mut stop_rx => return Ok(()),
        r = wait_for_broadcast(&mut origin_rx) => r?,
    };

    // Subscribe to the screen track within the broadcast.
    let track = broadcast_consumer
        .subscribe_track(&Track {
            name: SCREEN_TRACK_NAME.to_string(),
            priority: SCREEN_TRACK_PRIORITY,
        })
        .map_err(|e| Error::Transport(format!("moq subscribe_track: {e}")))?;

    info!("moq screen track subscribed");

    drain_track(track, &pkt_tx, &mut stop_rx).await;
    Ok(())
}

async fn wait_for_broadcast(
    origin: &mut moq_lite::OriginConsumer,
) -> Result<moq_lite::BroadcastConsumer> {
    loop {
        let (path, maybe_broadcast) = origin
            .announced()
            .await
            .ok_or_else(|| Error::Transport("origin closed before broadcast".into()))?;

        if let Some(broadcast) = maybe_broadcast {
            debug!(%path, "broadcast announced");
            return Ok(broadcast);
        }
    }
}

async fn drain_track(
    mut track: TrackConsumer,
    pkt_tx: &std::sync::mpsc::SyncSender<EncodedPacket>,
    stop_rx: &mut oneshot::Receiver<()>,
) {
    loop {
        let group = tokio::select! {
            biased;
            _ = &mut *stop_rx => break,
            r = track.next_group() => match r {
                Ok(Some(g)) => g,
                Ok(None) => break,
                Err(e) => { warn!(error = %e, "moq track error"); break; }
            }
        };

        drain_group(group, pkt_tx).await;
    }
}

async fn drain_group(
    mut group: moq_lite::GroupConsumer,
    pkt_tx: &std::sync::mpsc::SyncSender<EncodedPacket>,
) {
    let mut first = true;
    loop {
        match group.next_frame().await {
            Ok(Some(mut frame)) => match frame.read_all().await {
                Ok(data) => {
                    let pkt = EncodedPacket {
                        data,
                        timestamp_us: 0,
                        is_keyframe: first,
                    };
                    first = false;
                    if pkt_tx.try_send(pkt).is_err() {
                        debug!("decoder channel full, dropping frame");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "moq frame read error");
                    return;
                }
            },
            Ok(None) => return,
            Err(e) => {
                warn!(error = %e, "moq group error");
                return;
            }
        }
    }
}
