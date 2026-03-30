//! Opus audio decode thread.
//!
//! Reads `OpusPacket` from a tokio mpsc channel, decodes to f32 PCM,
//! sends `AudioFrame` to the playback thread via sync mpsc.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use pairwire_core::{Error, Result, config::AudioConfig};
use tokio::sync::mpsc as async_mpsc;
use tracing::{info, warn};

use crate::{
    OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE,
    frame::{AudioFrame, OpusPacket},
};

/// Handle to a running audio decode thread.
pub struct AudioDecodeThread {
    stop_flag: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl AudioDecodeThread {
    /// Spawn the decode thread.
    ///
    /// `pkt_rx`: encoded Opus packets from the network.
    /// Returns a handle and a sync receiver for decoded `AudioFrame`s.
    pub fn start(
        config: AudioConfig,
        pkt_rx: async_mpsc::Receiver<OpusPacket>,
    ) -> Result<(Self, mpsc::Receiver<AudioFrame>)> {
        let (frame_tx, frame_rx) = mpsc::sync_channel(16);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_thread = Arc::clone(&stop_flag);

        let handle = thread::Builder::new()
            .name("pairwire-audio-decode".into())
            .spawn(move || {
                if let Err(e) = decode_loop(config, pkt_rx, frame_tx, stop_flag_thread) {
                    warn!(error = %e, "audio decode thread error");
                }
                info!("audio decode thread stopped");
            })
            .map_err(|e| Error::Audio(format!("spawn decode thread: {e}")))?;

        Ok((
            Self {
                stop_flag,
                handle: Some(handle),
            },
            frame_rx,
        ))
    }

    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn decode_loop(
    config: AudioConfig,
    mut pkt_rx: async_mpsc::Receiver<OpusPacket>,
    frame_tx: mpsc::SyncSender<AudioFrame>,
    stop_flag: Arc<AtomicBool>,
) -> Result<()> {
    let mut decoder = opus::Decoder::new(
        OPUS_SAMPLE_RATE,
        if config.channels == 1 {
            opus::Channels::Mono
        } else {
            opus::Channels::Stereo
        },
    )
    .map_err(|e| Error::Audio(format!("opus decoder init: {e}")))?;

    info!(
        sample_rate = OPUS_SAMPLE_RATE,
        channels = config.channels,
        "audio decode thread started"
    );

    let frame_samples = OPUS_FRAME_SAMPLES * config.channels as usize;

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        let pkt = match pkt_rx.blocking_recv() {
            Some(p) => p,
            None => break,
        };

        let mut pcm = vec![0.0f32; frame_samples];
        let n = decoder
            .decode_float(&pkt.data, &mut pcm, false)
            .map_err(|e| Error::Audio(format!("opus decode: {e}")))?;
        pcm.truncate(n * config.channels as usize);

        let frame = AudioFrame {
            samples: pcm,
            sample_rate: OPUS_SAMPLE_RATE,
            channels: config.channels,
            timestamp_us: pkt.timestamp_us,
        };

        if frame_tx.send(frame).is_err() {
            break;
        }
    }

    Ok(())
}
