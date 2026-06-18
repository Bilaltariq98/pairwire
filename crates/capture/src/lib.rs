//! Screen capture and H.264 encoding/decoding pipeline.
//!
//! Feature flags:
//! - `capture`: real screen capture via scap (requires display hardware)
//! - `ffmpeg`: H.264 encode/decode via `FFmpeg` (requires system `FFmpeg` libs)
//!
//! Without feature flags, only `CaptureThread::start_fake()` is available.

pub mod capture_thread;
pub mod cursor_visibility;
pub mod decode_thread;
pub mod encode_thread;
pub mod frame;

pub use capture_thread::CaptureThread;
pub use cursor_visibility::ShowCursorFlag;
pub use frame::{DecodedFrame, EncodedPacket, RawFrame};

#[cfg(feature = "ffmpeg")]
pub use decode_thread::DecodeThread;
#[cfg(feature = "ffmpeg")]
pub use encode_thread::EncodeThread;

// ── Screen Recording permission helpers ──────────────────────────────────────
//
// These thin wrappers are gated on the `capture` feature (which is also when
// scap is compiled in). Callers that need to check or request the macOS Screen
// Recording TCC permission before starting a host session can call these
// without taking a direct dependency on scap.

/// Returns `true` if the current process already has Screen Recording
/// permission (macOS TCC). Does NOT prompt the user.
///
/// Wraps `scap::has_permission()` (`CGPreflightScreenCaptureAccess`).
#[cfg(feature = "capture")]
#[must_use]
pub fn screen_recording_permission_granted() -> bool {
    scap::has_permission()
}

/// Requests Screen Recording permission from the user (shows the TCC dialog on
/// first run; returns the cached result on subsequent runs). Returns `true` if
/// permission was granted.
///
/// Wraps `scap::request_permission()` (`CGRequestScreenCaptureAccess`).
#[cfg(feature = "capture")]
#[must_use]
pub fn request_screen_recording_permission() -> bool {
    scap::request_permission()
}

/// Returns `true` if scap screen capture is supported on this platform.
///
/// Wraps `scap::is_supported()`.
#[cfg(feature = "capture")]
#[must_use]
pub fn screen_capture_supported() -> bool {
    scap::is_supported()
}
