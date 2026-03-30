//! Screen capture and H.264 encoding pipeline.
//!
//! Architecture:
//! - `CaptureThread`: dedicated OS thread running scap, sends `RawFrame` via channel
//! - `EncodeThread`: dedicated OS thread running FFmpeg encoder, sends `EncodedPacket`
//! - `DecodeThread`: dedicated OS thread running FFmpeg decoder, sends `DecodedFrame`
//!
//! FFmpeg encoder/decoder contexts are NOT Send — they must stay on their thread.
//! Communicate with the tokio runtime via `tokio::sync::mpsc` channels.
//!
//! Implemented in P1.3 (capture), P1.4 (encode), P1.5 (decode).

pub mod frame;

pub use frame::{DecodedFrame, EncodedPacket, RawFrame};
