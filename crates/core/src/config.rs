//! Session and subsystem configuration types.

use serde::{Deserialize, Serialize};

/// Top-level session configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub capture: CaptureConfig,
    pub audio: AudioConfig,
    /// Maximum bitrate for screen sharing in bits/sec.
    pub max_video_bitrate: u32,
    /// Whether to allow the remote peer to control input.
    pub allow_remote_control: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            capture: CaptureConfig::default(),
            audio: AudioConfig::default(),
            max_video_bitrate: 5_000_000,
            allow_remote_control: false,
        }
    }
}

/// Screen capture and encoding configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureConfig {
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    /// H.264 encoder to try first. Falls back to libx264 if unavailable.
    pub preferred_encoder: VideoEncoder,
    /// Keyframe interval in seconds.
    pub keyframe_interval_secs: u32,
    /// Send a keepalive packet after this many consecutive static (unchanged) frames.
    ///
    /// Keeps the receiver's inactivity timer alive without sending real video data.
    /// Set to 0 to disable keepalives (not recommended).
    pub keepalive_after_frames: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            fps: 30,
            width: 1920,
            height: 1080,
            preferred_encoder: VideoEncoder::H264VideoToolbox,
            keyframe_interval_secs: 2,
            keepalive_after_frames: 60, // 2 seconds at 30fps
        }
    }
}

/// Video encoder preference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoEncoder {
    /// macOS hardware encoder (`ScreenCaptureKit` / `VideoToolbox`).
    H264VideoToolbox,
    /// Software fallback, available everywhere `FFmpeg` is built.
    H264Software,
}

/// Audio capture and encoding configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioConfig {
    pub sample_rate: u32,
    pub channels: u8,
    /// Opus frame duration in milliseconds (2.5, 5, 10, 20, 40, 60).
    pub frame_duration_ms: u32,
    /// Opus bitrate in bits/sec.
    pub bitrate: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 1,
            frame_duration_ms: 20,
            bitrate: 32_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postcard::{from_bytes, to_allocvec};

    #[test]
    fn session_config_round_trip() {
        let cfg = SessionConfig::default();
        let bytes = to_allocvec(&cfg).unwrap();
        let decoded: SessionConfig = from_bytes(&bytes).unwrap();
        assert_eq!(decoded, cfg);
    }
}
