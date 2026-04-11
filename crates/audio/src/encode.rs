//! Opus audio encode thread.
//!
//! Reads `AudioFrame` from a sync channel (OS thread → OS thread).
//! Resamples to 48kHz mono if needed, then encodes 20ms Opus frames.
//! Sends `OpusPacket` via tokio mpsc (OS thread → async task).
//!
//! Voice activity detection: frames below `SILENCE_THRESHOLD` RMS are skipped.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use bytes::Bytes;
use pairwire_core::{Error, Result, config::AudioConfig};
use tokio::sync::mpsc as async_mpsc;
use tracing::{debug, info, warn};

use crate::{
    OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE, SILENCE_THRESHOLD,
    frame::{AudioFrame, OpusPacket},
};

/// Handle to a running audio encode thread.
pub struct AudioEncodeThread {
    stop_flag: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl AudioEncodeThread {
    /// Spawn the encode thread.
    ///
    /// `frame_rx`: raw PCM frames from the capture thread.
    /// Returns a handle and a tokio receiver for encoded `OpusPacket`s.
    ///
    /// # Errors
    ///
    /// Returns an error if the OS thread cannot be spawned.
    pub fn start(
        config: AudioConfig,
        frame_rx: mpsc::Receiver<AudioFrame>,
    ) -> Result<(Self, async_mpsc::Receiver<OpusPacket>)> {
        let (pkt_tx, pkt_rx) = async_mpsc::channel(32);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_thread = Arc::clone(&stop_flag);

        let handle = thread::Builder::new()
            .name("pairwire-audio-encode".into())
            .spawn(move || {
                if let Err(e) = encode_loop(config, frame_rx, pkt_tx, stop_flag_thread) {
                    warn!(error = %e, "audio encode thread error");
                }
                info!("audio encode thread stopped");
            })
            .map_err(|e| Error::Audio(format!("spawn encode thread: {e}")))?;

        Ok((
            Self {
                stop_flag,
                handle: Some(handle),
            },
            pkt_rx,
        ))
    }

    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for AudioEncodeThread {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }
}

#[allow(clippy::needless_pass_by_value)] // args are consumed by the dedicated OS thread
fn encode_loop(
    config: AudioConfig,
    frame_rx: mpsc::Receiver<AudioFrame>,
    pkt_tx: async_mpsc::Sender<OpusPacket>,
    stop_flag: Arc<AtomicBool>,
) -> Result<()> {
    let encoder = opus::Encoder::new(
        OPUS_SAMPLE_RATE,
        if config.channels == 1 {
            opus::Channels::Mono
        } else {
            opus::Channels::Stereo
        },
        opus::Application::Voip,
    )
    .map_err(|e| Error::Audio(format!("opus encoder init: {e}")))?;

    // Set bitrate
    let mut encoder = encoder;
    #[allow(clippy::cast_possible_wrap)] // bitrate is always a small positive value
    encoder
        .set_bitrate(opus::Bitrate::Bits(config.bitrate as i32))
        .map_err(|e| Error::Audio(format!("opus set_bitrate: {e}")))?;

    info!(
        sample_rate = OPUS_SAMPLE_RATE,
        channels = config.channels,
        bitrate = config.bitrate,
        "audio encode thread started"
    );

    // Accumulate samples until we have a full 20ms frame.
    let mut sample_buf: Vec<f32> =
        Vec::with_capacity(OPUS_FRAME_SAMPLES * config.channels as usize);
    let mut timestamp_us: u64 = 0;
    let frame_us = (OPUS_FRAME_SAMPLES as u64 * 1_000_000) / u64::from(OPUS_SAMPLE_RATE);

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        let Ok(frame) = frame_rx.recv() else { break };

        // Resample to mono 48kHz if needed (simple downsample — good enough for voice).
        let samples = normalize_frame(&frame, config.channels);
        sample_buf.extend_from_slice(&samples);

        // Encode all complete 20ms frames from the buffer.
        let frame_len = OPUS_FRAME_SAMPLES * config.channels as usize;
        while sample_buf.len() >= frame_len {
            let chunk: Vec<f32> = sample_buf.drain(..frame_len).collect();

            // Voice activity detection — send silence marker instead of encoding.
            let rms = rms(&chunk);
            if rms < SILENCE_THRESHOLD {
                debug!(rms, "silence, sending marker");
                let marker = OpusPacket::silence(timestamp_us, 20);
                timestamp_us += frame_us;
                if pkt_tx.blocking_send(marker).is_err() {
                    return Ok(());
                }
                continue;
            }

            let mut out = vec![0u8; 4000];
            let encoded_len = encoder
                .encode_float(&chunk, &mut out)
                .map_err(|e| Error::Audio(format!("opus encode: {e}")))?;
            out.truncate(encoded_len);

            let pkt = OpusPacket {
                data: Bytes::from(out),
                timestamp_us,
                duration_ms: 20,
            };
            timestamp_us += frame_us;

            if pkt_tx.blocking_send(pkt).is_err() {
                return Ok(());
            }
        }
    }

    Ok(())
}

/// Downsample to the target channel count and resample to 48kHz if needed.
///
/// For simplicity: if source is stereo and target is mono, average channels.
/// Resampling is not implemented — cpal is configured to capture at 48kHz.
fn normalize_frame(frame: &AudioFrame, target_channels: u8) -> Vec<f32> {
    if frame.channels == target_channels {
        return frame.samples.clone();
    }
    // Stereo → mono: average left and right.
    if frame.channels == 2 && target_channels == 1 {
        return frame
            .samples
            .chunks_exact(2)
            .map(|c| (c[0] + c[1]) * 0.5)
            .collect();
    }
    // Mono → stereo: duplicate.
    if frame.channels == 1 && target_channels == 2 {
        return frame.samples.iter().flat_map(|&s| [s, s]).collect();
    }
    frame.samples.clone()
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    #[allow(clippy::cast_precision_loss)] // sample count fits in f32 for audio frame sizes
    let len = samples.len() as f32;
    (sum_sq / len).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_silence_is_zero() {
        assert_eq!(rms(&[0.0; 960]), 0.0);
    }

    #[test]
    fn rms_full_scale_sine() {
        // sin wave at amplitude 1.0 → RMS = 1/sqrt(2) ≈ 0.707
        let samples: Vec<f32> = (0..960)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI / 48.0).sin())
            .collect();
        let r = rms(&samples);
        assert!(
            (r - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01,
            "rms={r}"
        );
    }

    #[test]
    fn stereo_to_mono_averages() {
        let frame = AudioFrame {
            samples: vec![0.5, 0.3, 0.5, 0.3],
            sample_rate: 48_000,
            channels: 2,
            timestamp_us: 0,
        };
        let mono = normalize_frame(&frame, 1);
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn opus_encode_decode_round_trip() {
        // Encode a 20ms sine frame and decode it — sample count must match.
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let mut decoder = opus::Decoder::new(48_000, opus::Channels::Mono).unwrap();

        let input: Vec<f32> = (0..OPUS_FRAME_SAMPLES)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / 48000.0).sin() * 0.5)
            .collect();

        let mut encoded = vec![0u8; 4000];
        let len = encoder.encode_float(&input, &mut encoded).unwrap();
        encoded.truncate(len);

        let mut decoded = vec![0.0f32; OPUS_FRAME_SAMPLES];
        let n = decoder.decode_float(&encoded, &mut decoded, false).unwrap();
        assert_eq!(n, OPUS_FRAME_SAMPLES);
    }

    #[test]
    fn silence_detection_threshold() {
        // Silence (all zeros) should be below SILENCE_THRESHOLD.
        let silence = vec![0.0f32; OPUS_FRAME_SAMPLES];
        assert!(rms(&silence) < SILENCE_THRESHOLD);

        // A very quiet signal (0.001 amplitude) is still below.
        let quiet: Vec<f32> = (0..OPUS_FRAME_SAMPLES)
            .map(|i| (i as f32 * 0.01).sin() * 0.001)
            .collect();
        assert!(rms(&quiet) < SILENCE_THRESHOLD);

        // A normal voice signal (0.1 amplitude) is above.
        let voice: Vec<f32> = (0..OPUS_FRAME_SAMPLES)
            .map(|i| (i as f32 * 0.01).sin() * 0.1)
            .collect();
        assert!(rms(&voice) > SILENCE_THRESHOLD);
    }

    #[test]
    fn silence_marker_has_correct_fields() {
        let marker = OpusPacket::silence(42_000, 20);
        assert!(marker.is_silence());
        assert!(marker.data.is_empty());
        assert_eq!(marker.timestamp_us, 42_000);
        assert_eq!(marker.duration_ms, 20);
    }

    #[test]
    fn rms_empty_is_zero() {
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn normalize_same_channels_is_clone() {
        let frame = AudioFrame {
            samples: vec![0.1, 0.2, 0.3],
            sample_rate: 48_000,
            channels: 1,
            timestamp_us: 0,
        };
        let out = normalize_frame(&frame, 1);
        assert_eq!(out, frame.samples);
    }

    #[test]
    fn mono_to_stereo_duplicates() {
        let frame = AudioFrame {
            samples: vec![0.5, -0.5],
            sample_rate: 48_000,
            channels: 1,
            timestamp_us: 0,
        };
        let stereo = normalize_frame(&frame, 2);
        assert_eq!(stereo, vec![0.5, 0.5, -0.5, -0.5]);
    }
}
