//! Audio capture and Opus codec pipeline.
//!
//! Architecture:
//! - `AudioCaptureThread`: dedicated OS thread running cpal, sends PCM via channel
//! - `AudioEncodeThread`: encodes PCM to Opus frames (20ms per frame at 48kHz)
//! - `AudioDecodeThread`: decodes Opus frames back to PCM for playback
//! - `AudioPlaybackThread`: cpal output stream with a simple jitter buffer
//!
//! Audio always takes priority over video (moq-lite priority 0).
//! Opus handles packet loss gracefully — no retransmission needed.
//!
//! Voice activity detection: frames below the RMS silence threshold are not encoded.

pub mod capture;
pub mod decode;
pub mod encode;
pub mod frame;
pub mod playback;

pub use capture::{AudioCaptureThread, FakeAudioSource};
pub use decode::AudioDecodeThread;
pub use encode::AudioEncodeThread;
pub use frame::{AudioFrame, OpusPacket};
pub use playback::AudioPlaybackThread;

/// Opus sample rate — fixed at 48kHz per spec.
pub const OPUS_SAMPLE_RATE: u32 = 48_000;

/// Opus frame size for 20ms at 48kHz (samples per channel).
pub const OPUS_FRAME_SAMPLES: usize = 960;

/// RMS threshold below which a frame is considered silence and skipped.
pub const SILENCE_THRESHOLD: f32 = 0.001;
