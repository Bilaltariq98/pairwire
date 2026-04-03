//! Audio frame types for the capture/encode/decode pipeline.

use bytes::Bytes;

/// Raw PCM audio samples from the capture device.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    /// Interleaved f32 PCM samples.
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u8,
    /// Capture timestamp in microseconds.
    pub timestamp_us: u64,
}

impl AudioFrame {
    /// Number of samples per channel.
    #[must_use]
    pub fn samples_per_channel(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    /// Duration of this frame in milliseconds.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // sample count fits in f64 for audio frame sizes
    pub fn duration_ms(&self) -> f64 {
        (self.samples_per_channel() as f64 / f64::from(self.sample_rate)) * 1000.0
    }
}

/// An Opus-encoded audio packet.
#[derive(Debug, Clone)]
pub struct OpusPacket {
    pub data: Bytes,
    /// Presentation timestamp in microseconds.
    pub timestamp_us: u64,
    /// Duration of audio in this packet in milliseconds.
    pub duration_ms: u32,
}

impl OpusPacket {
    /// Returns true if this is a silence marker (empty data with valid timing).
    ///
    /// Silence markers keep the decoder's timestamp in sync without
    /// transmitting encoded audio.
    #[must_use]
    pub fn is_silence(&self) -> bool {
        self.data.is_empty()
    }

    /// Create a silence marker with the given timestamp and duration.
    #[must_use]
    pub fn silence(timestamp_us: u64, duration_ms: u32) -> Self {
        Self {
            data: Bytes::new(),
            timestamp_us,
            duration_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_frame_duration() {
        // 48kHz, 20ms frame = 960 samples per channel
        let frame = AudioFrame {
            samples: vec![0.0f32; 960],
            sample_rate: 48_000,
            channels: 1,
            timestamp_us: 0,
        };
        let duration = frame.duration_ms();
        assert!(
            (duration - 20.0).abs() < 0.01,
            "expected 20ms, got {duration}ms"
        );
    }

    #[test]
    fn silence_marker_is_silence() {
        let pkt = OpusPacket::silence(1000, 20);
        assert!(pkt.is_silence());
        assert_eq!(pkt.timestamp_us, 1000);
        assert_eq!(pkt.duration_ms, 20);
        assert!(pkt.data.is_empty());
    }

    #[test]
    fn non_silence_packet() {
        let pkt = OpusPacket {
            data: Bytes::from_static(&[0x01, 0x02]),
            timestamp_us: 2000,
            duration_ms: 20,
        };
        assert!(!pkt.is_silence());
    }
}
