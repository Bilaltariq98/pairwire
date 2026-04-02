//! Unified moq-lite session with hang media framing.
//!
//! Replaces the separate raw-QUIC screen transport and standalone moq-lite audio
//! session with a single moq-lite session publishing both video and audio tracks
//! in one broadcast, described by a hang catalog.
//!
//! ## Architecture
//!
//! ```text
//! Host:
//!   OriginProducer → BroadcastProducer → catalog.json  (codec/resolution metadata)
//!                                      → video0        (H.264, priority 3)
//!                                      → audio0        (Opus, priority 0)
//!   Server::accept(wt) — one moq-lite SETUP handshake
//!
//! Guest:
//!   Client::connect(wt) — reads catalog → subscribes to video0 + audio0
//!   OrderedConsumer handles latency, group ordering, frame timestamps
//! ```
//!
//! This eliminates the `UniStreamRouter` and `ScreenFrameHeader`. Both media
//! tracks share a single moq-lite session over the iroh connection.

use std::sync::Arc;
use std::time::Duration;

use bytes::Buf;
use hang::catalog::{
    AudioCodec, AudioConfig, Catalog, CatalogConsumer, Container, H264, VideoConfig,
};
use hang::container::{BufList, Frame, OrderedConsumer, OrderedProducer, Timestamp};
use moq_lite::{BroadcastProducer, Client, OriginProducer, Server, Track};
use pairwire_audio::OpusPacket;
use pairwire_capture::EncodedPacket;
use pairwire_core::{Error, Result};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, info, instrument, warn};

use crate::connection::PairwireConnection;
use crate::wt_session::IrohWtSession;

/// Broadcast path for the pairwire session.
const BROADCAST_PATH: &str = "pairwire/session";

/// Maximum latency before the consumer skips old groups.
const MAX_CONSUMER_LATENCY: Duration = Duration::from_millis(500);

// ── Configuration ────────────────────────────────────────────────────────────

/// Video stream metadata for the hang catalog.
#[derive(Debug, Clone)]
pub struct VideoStreamConfig {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub bitrate: u64,
}

/// Audio stream metadata for the hang catalog.
#[derive(Debug, Clone)]
pub struct AudioStreamConfig {
    pub sample_rate: u32,
    pub channel_count: u32,
    pub bitrate: u64,
}

// ── Publisher (host side) ────────────────────────────────────────────────────

/// Handle for stopping the unified moq session publisher.
pub struct MoqSessionPublisherHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl MoqSessionPublisherHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

/// Publishes screen frames and optional audio over a single moq-lite session
/// using hang media framing and a catalog track.
pub struct MoqSessionPublisher {
    conn: Arc<PairwireConnection>,
}

impl MoqSessionPublisher {
    #[must_use]
    pub fn new(conn: Arc<PairwireConnection>) -> Self {
        Self { conn }
    }

    /// Spawn the publisher.
    ///
    /// `video_rx` receives H.264 encoded packets from the capture/encode pipeline.
    /// `audio_rx` receives Opus packets from the audio pipeline (pass `None` to skip audio).
    #[instrument(skip_all)]
    pub fn spawn(
        self,
        video_config: VideoStreamConfig,
        audio_config: Option<AudioStreamConfig>,
        video_rx: mpsc::Receiver<EncodedPacket>,
        audio_rx: Option<mpsc::Receiver<OpusPacket>>,
    ) -> MoqSessionPublisherHandle {
        let (stop_tx, stop_rx) = oneshot::channel();

        let join = tokio::spawn(async move {
            if let Err(e) = publish_session(
                self.conn,
                video_config,
                audio_config,
                video_rx,
                audio_rx,
                stop_rx,
            )
            .await
            {
                warn!(error = %e, "moq session publisher error");
            }
            info!("moq session publisher stopped");
        });

        MoqSessionPublisherHandle { stop_tx, join }
    }
}

async fn publish_session(
    conn: Arc<PairwireConnection>,
    video_config: VideoStreamConfig,
    audio_config: Option<AudioStreamConfig>,
    video_rx: mpsc::Receiver<EncodedPacket>,
    audio_rx: Option<mpsc::Receiver<OpusPacket>>,
    stop_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let wt = IrohWtSession::new(conn.raw_conn().clone());

    // Build catalog describing available tracks.
    let mut catalog = Catalog::default();

    let video_moq_track = catalog.video.create_track(
        "",
        VideoConfig {
            codec: H264 {
                profile: 0x64, // High profile
                constraints: 0x00,
                level: 0x1f,  // Level 3.1
                inline: true, // SPS/PPS in-band (avc3)
            }
            .into(),
            coded_width: Some(video_config.width),
            coded_height: Some(video_config.height),
            bitrate: Some(video_config.bitrate),
            framerate: Some(video_config.fps),
            optimize_for_latency: Some(true),
            container: Container::Legacy,
            description: None,
            display_ratio_width: None,
            display_ratio_height: None,
            jitter: None,
        },
    );

    let audio_moq_track = audio_config.map(|ac| {
        catalog.audio.create_track(
            "",
            AudioConfig {
                codec: AudioCodec::Opus,
                sample_rate: ac.sample_rate,
                channel_count: ac.channel_count,
                bitrate: Some(ac.bitrate),
                description: None,
                container: Container::Legacy,
                jitter: None,
            },
        )
    });

    // Build broadcast with catalog + media tracks.
    let origin = OriginProducer::new();
    let mut broadcast = BroadcastProducer::new();

    // Catalog track — priority 100 (consumed first by subscribers).
    let mut catalog_track = broadcast
        .create_track(Catalog::default_track())
        .map_err(|e| Error::Transport(format!("create catalog track: {e}")))?;

    // Video track.
    let video_track = broadcast
        .create_track(video_moq_track)
        .map_err(|e| Error::Transport(format!("create video track: {e}")))?;
    let video_producer = OrderedProducer::new(video_track);

    // Audio track (optional).
    let audio_producer = audio_moq_track
        .map(|t| {
            broadcast
                .create_track(t)
                .map(OrderedProducer::new)
                .map_err(|e| Error::Transport(format!("create audio track: {e}")))
        })
        .transpose()?;

    // Publish catalog JSON.
    let catalog_bytes = catalog
        .to_vec()
        .map_err(|e| Error::Transport(format!("serialize catalog: {e}")))?;
    let mut cat_group = catalog_track
        .append_group()
        .map_err(|e| Error::Transport(format!("catalog append_group: {e}")))?;
    cat_group
        .write_frame(bytes::Bytes::from(catalog_bytes))
        .map_err(|e| Error::Transport(format!("catalog write_frame: {e}")))?;
    cat_group
        .finish()
        .map_err(|e| Error::Transport(format!("catalog finish: {e}")))?;

    origin.publish_broadcast(BROADCAST_PATH, broadcast.consume());

    // moq-lite SETUP handshake — host acts as server.
    let _session = Server::new()
        .with_publish(origin.consume())
        .accept(wt)
        .await
        .map_err(|e| Error::Transport(format!("moq server accept: {e}")))?;

    info!("moq session publisher ready (video + audio)");

    // Spawn feed tasks for each media track.
    let video_task = tokio::spawn(feed_video(video_producer, video_rx));
    let audio_task = audio_producer.map(|ap| {
        let rx = audio_rx.expect("audio_rx required when audio_config is Some");
        tokio::spawn(feed_audio(ap, rx))
    });

    // Wait for stop signal.
    let _ = stop_rx.await;

    video_task.abort();
    if let Some(task) = audio_task {
        task.abort();
    }

    Ok(())
}

async fn feed_video(mut producer: OrderedProducer, mut video_rx: mpsc::Receiver<EncodedPacket>) {
    while let Some(pkt) = video_rx.recv().await {
        // Keepalives carry no video data — skip.
        if pkt.is_keepalive {
            debug!("keepalive, skipping");
            continue;
        }

        // Start a new group on each keyframe.
        if pkt.is_keyframe
            && let Err(e) = producer.keyframe()
        {
            warn!(error = %e, "video keyframe error");
            return;
        }

        let timestamp = match Timestamp::from_micros(pkt.timestamp_us) {
            Ok(ts) => ts,
            Err(e) => {
                warn!(error = %e, "invalid video timestamp");
                continue;
            }
        };

        let frame = Frame {
            timestamp,
            payload: BufList::from_iter(vec![pkt.data]),
        };

        if let Err(e) = producer.write(frame) {
            warn!(error = %e, "video write error");
            return;
        }
    }

    let _ = producer.finish();
    info!("video feed ended");
}

async fn feed_audio(mut producer: OrderedProducer, mut audio_rx: mpsc::Receiver<OpusPacket>) {
    while let Some(pkt) = audio_rx.recv().await {
        // Each Opus packet is its own group (audio frames are independent).
        if let Err(e) = producer.keyframe() {
            warn!(error = %e, "audio keyframe error");
            return;
        }

        let timestamp = match Timestamp::from_micros(pkt.timestamp_us) {
            Ok(ts) => ts,
            Err(e) => {
                warn!(error = %e, "invalid audio timestamp");
                continue;
            }
        };

        let frame = Frame {
            timestamp,
            payload: BufList::from_iter(vec![pkt.data]),
        };

        if let Err(e) = producer.write(frame) {
            warn!(error = %e, "audio write error");
            return;
        }
    }

    let _ = producer.finish();
    info!("audio feed ended");
}

// ── Subscriber (guest side) ──────────────────────────────────────────────────

/// Handle for stopping the unified moq session subscriber.
pub struct MoqSessionSubscriberHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl MoqSessionSubscriberHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

/// Subscribes to the host's moq-lite session, reads the hang catalog, and
/// feeds received video/audio frames to output channels.
pub struct MoqSessionSubscriber {
    conn: Arc<PairwireConnection>,
}

impl MoqSessionSubscriber {
    #[must_use]
    pub fn new(conn: Arc<PairwireConnection>) -> Self {
        Self { conn }
    }

    /// Spawn the subscriber.
    ///
    /// `video_tx` receives decoded H.264 packets for the decode thread.
    /// `audio_tx` receives Opus packets for the audio pipeline (pass `None` to skip audio).
    #[instrument(skip_all)]
    pub fn spawn(
        self,
        video_tx: std::sync::mpsc::SyncSender<EncodedPacket>,
        audio_tx: Option<mpsc::Sender<OpusPacket>>,
    ) -> MoqSessionSubscriberHandle {
        let (stop_tx, stop_rx) = oneshot::channel();

        let join = tokio::spawn(async move {
            if let Err(e) = subscribe_session(self.conn, video_tx, audio_tx, stop_rx).await {
                warn!(error = %e, "moq session subscriber error");
            }
            info!("moq session subscriber stopped");
        });

        MoqSessionSubscriberHandle { stop_tx, join }
    }
}

async fn subscribe_session(
    conn: Arc<PairwireConnection>,
    video_tx: std::sync::mpsc::SyncSender<EncodedPacket>,
    audio_tx: Option<mpsc::Sender<OpusPacket>>,
    mut stop_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let wt = IrohWtSession::new(conn.raw_conn().clone());

    let origin = OriginProducer::new();

    // moq-lite SETUP handshake — guest acts as client.
    let _session = Client::new()
        .with_consume(origin.clone())
        .connect(wt)
        .await
        .map_err(|e| Error::Transport(format!("moq client connect: {e}")))?;

    info!("moq session subscriber connected");

    // Wait for the broadcast to be announced.
    let mut origin_rx = origin.consume();
    let broadcast = tokio::select! {
        biased;
        _ = &mut stop_rx => return Ok(()),
        r = wait_for_broadcast(&mut origin_rx) => r?,
    };

    // Read the catalog to discover available tracks.
    let catalog_track = broadcast
        .subscribe_track(&Catalog::default_track())
        .map_err(|e| Error::Transport(format!("subscribe catalog: {e}")))?;

    let mut catalog_consumer = CatalogConsumer::new(catalog_track);
    let catalog = tokio::select! {
        biased;
        _ = &mut stop_rx => return Ok(()),
        r = catalog_consumer.next() => {
            r.map_err(|e| Error::Transport(format!("read catalog: {e}")))?
                .ok_or_else(|| Error::Transport("catalog track ended before first entry".into()))?
        }
    };

    info!(
        video_tracks = catalog.video.renditions.len(),
        audio_tracks = catalog.audio.renditions.len(),
        "catalog received"
    );

    // Subscribe to the first video rendition.
    let video_task = if let Some((name, _config)) = catalog.video.renditions.iter().next() {
        let track = broadcast
            .subscribe_track(&Track {
                name: name.clone(),
                priority: 3,
            })
            .map_err(|e| Error::Transport(format!("subscribe video: {e}")))?;

        let consumer = OrderedConsumer::new(track, MAX_CONSUMER_LATENCY);
        info!(track = %name, "subscribed to video track");
        Some(tokio::spawn(drain_video(consumer, video_tx)))
    } else {
        warn!("no video tracks in catalog");
        None
    };

    // Subscribe to the first audio rendition (if audio channel provided).
    let audio_task = if let (Some(tx), Some((name, _config))) =
        (&audio_tx, catalog.audio.renditions.iter().next())
    {
        let track = broadcast
            .subscribe_track(&Track {
                name: name.clone(),
                priority: 0,
            })
            .map_err(|e| Error::Transport(format!("subscribe audio: {e}")))?;

        let consumer = OrderedConsumer::new(track, MAX_CONSUMER_LATENCY);
        info!(track = %name, "subscribed to audio track");
        Some(tokio::spawn(drain_audio(consumer, tx.clone())))
    } else {
        None
    };

    // Wait for stop signal.
    let _ = stop_rx.await;

    if let Some(task) = video_task {
        task.abort();
    }
    if let Some(task) = audio_task {
        task.abort();
    }

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

async fn drain_video(
    mut consumer: OrderedConsumer,
    pkt_tx: std::sync::mpsc::SyncSender<EncodedPacket>,
) {
    loop {
        match consumer.read().await {
            Ok(Some(frame)) => {
                let is_keyframe = frame.is_keyframe();
                let timestamp_us = frame.timestamp.as_micros() as u64;
                let mut payload = frame.payload;
                let data = payload.copy_to_bytes(payload.remaining());
                let pkt = EncodedPacket {
                    data,
                    timestamp_us,
                    is_keyframe,
                    is_keepalive: false,
                };
                if pkt_tx.try_send(pkt).is_err() {
                    debug!("decoder channel full, dropping frame");
                }
            }
            Ok(None) => {
                info!("video track ended");
                break;
            }
            Err(e) => {
                warn!(error = %e, "video track error");
                break;
            }
        }
    }
}

async fn drain_audio(mut consumer: OrderedConsumer, pkt_tx: mpsc::Sender<OpusPacket>) {
    loop {
        match consumer.read().await {
            Ok(Some(frame)) => {
                let mut payload = frame.payload;
                let data = payload.copy_to_bytes(payload.remaining());
                let pkt = OpusPacket {
                    data,
                    timestamp_us: frame.timestamp.as_micros() as u64,
                    duration_ms: 20, // standard Opus frame duration
                };
                if pkt_tx.send(pkt).await.is_err() {
                    break;
                }
            }
            Ok(None) => {
                info!("audio track ended");
                break;
            }
            Err(e) => {
                warn!(error = %e, "audio track error");
                break;
            }
        }
    }
}
