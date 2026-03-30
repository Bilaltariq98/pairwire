//! moq-lite audio track: publish and subscribe to Opus audio packets.
//!
//! The host publishes a BroadcastProducer with a single "audio" track (priority 0).
//! Each Opus packet is its own Group (audio frames are independent — no keyframe concept).
//! The guest subscribes and feeds received packets to the decode channel.
//!
//! Priority 0 = highest — audio always takes precedence over screen frames (priority 3).

use std::sync::Arc;

use moq_lite::{BroadcastProducer, Client, OriginProducer, Server, Track};
use pairwire_audio::OpusPacket;
use pairwire_core::{Error, Result};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tracing::{debug, info, instrument, warn};

use crate::{connection::PairwireConnection, wt_session::IrohWtSession};

/// Name of the audio track within the broadcast.
pub const AUDIO_TRACK_NAME: &str = "audio";

/// moq-lite track priority for audio (0 = highest).
pub const AUDIO_TRACK_PRIORITY: u8 = 0;

/// Path under which the audio broadcast is published.
pub const AUDIO_BROADCAST_PATH: &str = "pairwire/audio";

// ── Publisher (host side) ────────────────────────────────────────────────────

pub struct MoqAudioPublisherHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl MoqAudioPublisherHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

/// Publishes Opus audio packets over a moq-lite track (priority 0).
///
/// Each packet is its own Group — audio frames are independent.
pub struct MoqAudioPublisher {
    conn: Arc<PairwireConnection>,
}

impl MoqAudioPublisher {
    pub fn new(conn: Arc<PairwireConnection>) -> Self {
        Self { conn }
    }

    #[instrument(skip(self, pkt_rx))]
    pub fn spawn(self, pkt_rx: mpsc::Receiver<OpusPacket>) -> MoqAudioPublisherHandle {
        let (stop_tx, stop_rx) = oneshot::channel();
        let join = tokio::spawn(async move {
            if let Err(e) = audio_publish_loop(self.conn, pkt_rx, stop_rx).await {
                warn!(error = %e, "moq audio publisher error");
            }
            info!("moq audio publisher stopped");
        });
        MoqAudioPublisherHandle { stop_tx, join }
    }
}

async fn audio_publish_loop(
    conn: Arc<PairwireConnection>,
    mut pkt_rx: mpsc::Receiver<OpusPacket>,
    mut stop_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let wt = IrohWtSession::new(conn.raw_conn().clone());

    let origin = OriginProducer::new();
    let mut broadcast = BroadcastProducer::new();
    let mut track = broadcast
        .create_track(Track {
            name: AUDIO_TRACK_NAME.to_string(),
            priority: AUDIO_TRACK_PRIORITY,
        })
        .map_err(|e| Error::Transport(format!("moq audio create_track: {e}")))?;

    origin.publish_broadcast(AUDIO_BROADCAST_PATH, broadcast.consume());

    let _session = Server::new()
        .with_publish(origin.consume())
        .accept(wt)
        .await
        .map_err(|e| Error::Transport(format!("moq audio server accept: {e}")))?;

    info!("moq audio publisher ready");

    loop {
        tokio::select! {
            biased;
            _ = &mut stop_rx => break,
            pkt = pkt_rx.recv() => {
                let Some(pkt) = pkt else { break };

                // Each Opus packet is its own group — audio frames are independent.
                let mut group = track
                    .append_group()
                    .map_err(|e| Error::Transport(format!("moq audio append_group: {e}")))?;

                group
                    .write_frame(pkt.data)
                    .map_err(|e| Error::Transport(format!("moq audio write_frame: {e}")))?;

                let _ = group.finish();
                debug!("audio packet published");
            }
        }
    }

    Ok(())
}

// ── Subscriber (guest side) ──────────────────────────────────────────────────

pub struct MoqAudioSubscriberHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl MoqAudioSubscriberHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

/// Subscribes to the host's audio track and feeds packets to the decode channel.
pub struct MoqAudioSubscriber {
    conn: Arc<PairwireConnection>,
}

impl MoqAudioSubscriber {
    pub fn new(conn: Arc<PairwireConnection>) -> Self {
        Self { conn }
    }

    #[instrument(skip(self, pkt_tx))]
    pub fn spawn(self, pkt_tx: mpsc::Sender<OpusPacket>) -> MoqAudioSubscriberHandle {
        let (stop_tx, stop_rx) = oneshot::channel();
        let join = tokio::spawn(async move {
            if let Err(e) = audio_subscribe_loop(self.conn, pkt_tx, stop_rx).await {
                warn!(error = %e, "moq audio subscriber error");
            }
            info!("moq audio subscriber stopped");
        });
        MoqAudioSubscriberHandle { stop_tx, join }
    }
}

async fn audio_subscribe_loop(
    conn: Arc<PairwireConnection>,
    pkt_tx: mpsc::Sender<OpusPacket>,
    mut stop_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let wt = IrohWtSession::new(conn.raw_conn().clone());
    let origin = OriginProducer::new();

    let _session = Client::new()
        .with_consume(origin.clone())
        .connect(wt)
        .await
        .map_err(|e| Error::Transport(format!("moq audio client connect: {e}")))?;

    info!("moq audio subscriber connected");

    let mut origin_rx = origin.consume();
    let broadcast = tokio::select! {
        biased;
        _ = &mut stop_rx => return Ok(()),
        r = wait_for_audio_broadcast(&mut origin_rx) => r?,
    };

    let mut track = broadcast
        .subscribe_track(&Track {
            name: AUDIO_TRACK_NAME.to_string(),
            priority: AUDIO_TRACK_PRIORITY,
        })
        .map_err(|e| Error::Transport(format!("moq audio subscribe_track: {e}")))?;

    info!("moq audio track subscribed");

    loop {
        let group = tokio::select! {
            biased;
            _ = &mut stop_rx => break,
            r = track.next_group() => match r {
                Ok(Some(g)) => g,
                Ok(None) => break,
                Err(e) => { warn!(error = %e, "moq audio track error"); break; }
            }
        };

        if let Some(pkt) = read_audio_group(group).await
            && pkt_tx.send(pkt).await.is_err()
        {
            break;
        }
    }

    Ok(())
}

async fn wait_for_audio_broadcast(
    origin: &mut moq_lite::OriginConsumer,
) -> Result<moq_lite::BroadcastConsumer> {
    loop {
        let (path, maybe_broadcast) = origin
            .announced()
            .await
            .ok_or_else(|| Error::Transport("audio origin closed".into()))?;

        if let Some(broadcast) = maybe_broadcast {
            debug!(%path, "audio broadcast announced");
            return Ok(broadcast);
        }
    }
}

async fn read_audio_group(mut group: moq_lite::GroupConsumer) -> Option<OpusPacket> {
    match group.next_frame().await {
        Ok(Some(mut frame)) => match frame.read_all().await {
            Ok(data) => Some(OpusPacket {
                data,
                timestamp_us: 0,
                duration_ms: 20,
            }),
            Err(e) => {
                warn!(error = %e, "moq audio frame read error");
                None
            }
        },
        Ok(None) => None,
        Err(e) => {
            warn!(error = %e, "moq audio group error");
            None
        }
    }
}
