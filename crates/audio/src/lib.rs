//! Audio capture and Opus codec pipeline.
//!
//! Architecture:
//! - `AudioCaptureThread`: dedicated OS thread running cpal, sends PCM samples via channel
//! - `AudioEncodeThread`: encodes PCM to Opus frames (20ms per frame at 48kHz)
//! - `AudioDecodeThread`: decodes Opus frames back to PCM for playback
//!
//! Audio always takes priority over video (moq-lite priority 0).
//! Opus handles packet loss gracefully — no retransmission needed.
//!
//! Implemented in Phase 3.

pub mod frame;

pub use frame::{AudioFrame, OpusPacket};
