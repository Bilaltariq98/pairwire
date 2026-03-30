//! Test infrastructure: fake frame/audio sources and in-memory channel pairs.
//!
//! Gated behind `#[cfg(any(test, feature = "test-support"))]` in lib.rs.
//! Use these in unit and integration tests to avoid needing real hardware.

use crate::config::{AudioConfig, CaptureConfig};
use bytes::Bytes;

/// A synthetic BGRA frame source for testing the capture/encode pipeline.
///
/// Generates frames with a solid color that changes each frame (cycling hue),
/// making it easy to verify that distinct frames are produced.
pub struct FakeFrameSource {
    pub width: u32,
    pub height: u32,
    frame_count: u64,
}

impl FakeFrameSource {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            frame_count: 0,
        }
    }

    pub fn from_config(cfg: &CaptureConfig) -> Self {
        Self::new(cfg.width, cfg.height)
    }

    /// Generate the next synthetic BGRA frame.
    ///
    /// Each frame has a unique solid color derived from the frame counter,
    /// so consecutive frames are always different (useful for encoder tests).
    pub fn next_frame(&mut self) -> (Bytes, u64) {
        let n = self.frame_count;
        self.frame_count += 1;

        // Cycle through colors: vary blue channel with frame index
        let b = (n % 256) as u8;
        let g = ((n / 256) % 256) as u8;
        let r = ((n / 65536) % 256) as u8;

        let pixel_count = (self.width * self.height) as usize;
        let mut data = vec![0u8; pixel_count * 4];
        for i in 0..pixel_count {
            data[i * 4] = b; // B
            data[i * 4 + 1] = g; // G
            data[i * 4 + 2] = r; // R
            data[i * 4 + 3] = 255; // A
        }

        let timestamp_us = n * 33_333; // ~30fps
        (Bytes::from(data), timestamp_us)
    }

    /// Generate a static frame (all pixels identical) — useful for skip-static tests.
    pub fn static_frame(&self) -> (Bytes, u64) {
        let pixel_count = (self.width * self.height) as usize;
        let data = vec![0x80u8; pixel_count * 4];
        (Bytes::from(data), 0)
    }

    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }
}

/// A synthetic PCM audio source for testing the audio pipeline.
///
/// Generates either silence or a sine wave at a configurable frequency.
pub struct FakeAudioSource {
    pub sample_rate: u32,
    pub channels: u8,
    sample_index: u64,
    /// Frequency of the sine wave in Hz. 0 = silence.
    pub frequency_hz: f32,
}

impl FakeAudioSource {
    pub fn silence(cfg: &AudioConfig) -> Self {
        Self {
            sample_rate: cfg.sample_rate,
            channels: cfg.channels,
            sample_index: 0,
            frequency_hz: 0.0,
        }
    }

    pub fn sine(cfg: &AudioConfig, frequency_hz: f32) -> Self {
        Self {
            sample_rate: cfg.sample_rate,
            channels: cfg.channels,
            sample_index: 0,
            frequency_hz,
        }
    }

    /// Generate `duration_ms` milliseconds of audio samples.
    pub fn next_samples(&mut self, duration_ms: u32) -> Vec<f32> {
        let sample_count = (self.sample_rate as f64 * duration_ms as f64 / 1000.0).round() as usize;
        let total = sample_count * self.channels as usize;
        let mut out = Vec::with_capacity(total);

        for i in 0..sample_count {
            let sample = if self.frequency_hz > 0.0 {
                let t = (self.sample_index + i as u64) as f32 / self.sample_rate as f32;
                (2.0 * std::f32::consts::PI * self.frequency_hz * t).sin() * 0.5
            } else {
                0.0
            };
            for _ in 0..self.channels {
                out.push(sample);
            }
        }

        self.sample_index += sample_count as u64;
        out
    }
}

/// A bidirectional in-memory channel pair for testing send/receive patterns.
///
/// Uses `std::sync::mpsc` to keep `core` free of the tokio dependency.
/// For async tests, use `tokio::sync::mpsc` directly in the test crate.
pub struct LoopbackChannel<T> {
    pub sender: std::sync::mpsc::SyncSender<T>,
    pub receiver: std::sync::mpsc::Receiver<T>,
}

impl<T> LoopbackChannel<T> {
    pub fn new(capacity: usize) -> (std::sync::mpsc::SyncSender<T>, std::sync::mpsc::Receiver<T>) {
        std::sync::mpsc::sync_channel(capacity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AudioConfig;

    #[test]
    fn fake_frame_source_correct_size() {
        let mut src = FakeFrameSource::new(1920, 1080);
        let (frame, _ts) = src.next_frame();
        assert_eq!(frame.len(), 1920 * 1080 * 4);
    }

    #[test]
    fn fake_frame_source_frames_differ() {
        let mut src = FakeFrameSource::new(64, 64);
        let (f1, _) = src.next_frame();
        let (f2, _) = src.next_frame();
        assert_ne!(f1, f2, "consecutive frames should differ");
    }

    #[test]
    fn fake_frame_source_static_is_stable() {
        let src = FakeFrameSource::new(64, 64);
        let (f1, _) = src.static_frame();
        let (f2, _) = src.static_frame();
        assert_eq!(f1, f2, "static frames should be identical");
    }

    #[test]
    fn fake_audio_source_sample_count() {
        let cfg = AudioConfig::default(); // 48kHz, 1ch, 20ms
        let mut src = FakeAudioSource::silence(&cfg);
        let samples = src.next_samples(20);
        // 48000 * 20 / 1000 = 960 samples * 1 channel
        assert_eq!(samples.len(), 960);
        assert!(samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn fake_audio_source_sine_nonzero() {
        let cfg = AudioConfig::default();
        let mut src = FakeAudioSource::sine(&cfg, 440.0);
        let samples = src.next_samples(20);
        assert!(samples.iter().any(|&s| s != 0.0));
    }
}
