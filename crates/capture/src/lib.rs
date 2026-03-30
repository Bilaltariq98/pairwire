//! Screen capture and H.264 encoding/decoding pipeline.
//!
//! Feature flags:
//! - `capture`: real screen capture via scap (requires display hardware)
//! - `ffmpeg`: H.264 encode/decode via `FFmpeg` (requires system `FFmpeg` libs)
//!
//! Without feature flags, only `CaptureThread::start_fake()` is available.

pub mod capture_thread;
pub mod decode_thread;
pub mod encode_thread;
pub mod frame;

pub use capture_thread::CaptureThread;
pub use frame::{DecodedFrame, EncodedPacket, RawFrame};

#[cfg(feature = "ffmpeg")]
pub use decode_thread::DecodeThread;
#[cfg(feature = "ffmpeg")]
pub use encode_thread::EncodeThread;
