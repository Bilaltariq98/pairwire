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

impl SessionConfig {
    /// Validates all sub-configs. Call at session startup.
    pub fn validate(&self) -> crate::Result<()> {
        self.capture.validate()?;
        self.audio.validate()?;
        Ok(())
    }
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

impl CaptureConfig {
    /// Validates that all fields have sane values.
    ///
    /// Returns an error describing the first invalid field found.
    pub fn validate(&self) -> crate::Result<()> {
        if self.fps == 0 {
            return Err(crate::Error::InvalidConfig("CaptureConfig: fps must be > 0".into()));
        }
        if self.fps > 240 {
            return Err(crate::Error::InvalidConfig(
                "CaptureConfig: fps must be <= 240".into(),
            ));
        }
        if self.width == 0 {
            return Err(crate::Error::InvalidConfig(
                "CaptureConfig: width must be > 0".into(),
            ));
        }
        if self.height == 0 {
            return Err(crate::Error::InvalidConfig(
                "CaptureConfig: height must be > 0".into(),
            ));
        }
        if !self.width.is_multiple_of(2) {
            return Err(crate::Error::InvalidConfig(
                "CaptureConfig: width must be divisible by 2 for H.264".into(),
            ));
        }
        if !self.height.is_multiple_of(2) {
            return Err(crate::Error::InvalidConfig(
                "CaptureConfig: height must be divisible by 2 for H.264".into(),
            ));
        }
        Ok(())
    }
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

impl AudioConfig {
    /// Valid Opus frame durations in milliseconds.
    const VALID_FRAME_DURATIONS: [u32; 6] = [3, 5, 10, 20, 40, 60];

    /// Validates that all fields have sane values.
    ///
    /// Returns an error describing the first invalid field found.
    pub fn validate(&self) -> crate::Result<()> {
        if self.sample_rate == 0 {
            return Err(crate::Error::InvalidConfig(
                "AudioConfig: sample_rate must be > 0".into(),
            ));
        }
        if self.channels == 0 || self.channels > 2 {
            return Err(crate::Error::InvalidConfig(
                "AudioConfig: channels must be 1 (mono) or 2 (stereo)".into(),
            ));
        }
        if !Self::VALID_FRAME_DURATIONS.contains(&self.frame_duration_ms) {
            return Err(crate::Error::InvalidConfig(format!(
                "AudioConfig: frame_duration_ms must be one of {:?}",
                Self::VALID_FRAME_DURATIONS
            )));
        }
        if self.bitrate == 0 {
            return Err(crate::Error::InvalidConfig(
                "AudioConfig: bitrate must be > 0".into(),
            ));
        }
        Ok(())
    }
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

    // --- CaptureConfig validation ---

    #[test]
    fn capture_default_is_valid() {
        CaptureConfig::default().validate().unwrap();
    }

    #[test]
    fn capture_rejects_zero_fps() {
        let cfg = CaptureConfig { fps: 0, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn capture_rejects_excessive_fps() {
        let cfg = CaptureConfig { fps: 241, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn capture_rejects_zero_width() {
        let cfg = CaptureConfig { width: 0, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn capture_rejects_zero_height() {
        let cfg = CaptureConfig { height: 0, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn capture_rejects_odd_width() {
        let cfg = CaptureConfig { width: 1921, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn capture_rejects_odd_height() {
        let cfg = CaptureConfig { height: 1081, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn capture_accepts_valid_custom() {
        let cfg = CaptureConfig {
            fps: 60,
            width: 1280,
            height: 720,
            ..Default::default()
        };
        cfg.validate().unwrap();
    }

    // --- AudioConfig validation ---

    #[test]
    fn audio_default_is_valid() {
        AudioConfig::default().validate().unwrap();
    }

    #[test]
    fn audio_rejects_zero_sample_rate() {
        let cfg = AudioConfig { sample_rate: 0, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn audio_rejects_zero_channels() {
        let cfg = AudioConfig { channels: 0, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn audio_rejects_three_channels() {
        let cfg = AudioConfig { channels: 3, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn audio_accepts_stereo() {
        let cfg = AudioConfig { channels: 2, ..Default::default() };
        cfg.validate().unwrap();
    }

    #[test]
    fn audio_rejects_invalid_frame_duration() {
        let cfg = AudioConfig { frame_duration_ms: 15, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn audio_rejects_zero_bitrate() {
        let cfg = AudioConfig { bitrate: 0, ..Default::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn audio_accepts_all_valid_durations() {
        for &dur in &AudioConfig::VALID_FRAME_DURATIONS {
            let cfg = AudioConfig { frame_duration_ms: dur, ..Default::default() };
            cfg.validate().unwrap();
        }
    }

    // --- SessionConfig validation delegates ---

    #[test]
    fn session_config_default_is_valid() {
        SessionConfig::default().validate().unwrap();
    }

    #[test]
    fn session_config_catches_bad_capture() {
        let mut cfg = SessionConfig::default();
        cfg.capture.fps = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn session_config_catches_bad_audio() {
        let mut cfg = SessionConfig::default();
        cfg.audio.channels = 0;
        assert!(cfg.validate().is_err());
    }
}
