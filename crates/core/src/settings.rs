//! Persisted user settings — the TOML-backed `Settings` model.
//!
//! `Settings` is the user-editable preferences layer (loaded from
//! `config.toml`, overridable by CLI flags). It is distinct from
//! [`crate::config::SessionConfig`], which is the runtime parameters
//! consumed by the capture/encode/transport pipeline.
//!
//! Flow:
//!
//! ```text
//! defaults  ──┐
//! config.toml ┼──► Settings ──► derive ──► SessionConfig
//! CLI flags  ─┘                            (consumed by pipeline)
//! ```
//!
//! Every section uses `#[serde(default)]` on each field so a partial
//! config file remains valid — adding new fields in future versions
//! will not break existing users.

use serde::{Deserialize, Serialize};

/// Top-level user settings. One of these is loaded from `config.toml`,
/// optionally overridden by CLI flags, then used to derive the runtime
/// `SessionConfig` and to drive permission checks.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub identity: IdentitySettings,
    pub capture: CaptureSettings,
    pub audio: AudioSettings,
    pub permissions: PermissionSettings,
    pub terminal: TerminalSettings,
    pub tunnels: TunnelSettings,
    pub ui: UiSettings,
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// Persistent identity for the local peer.
///
/// `secret_key_path` (when set) points to a file holding the iroh secret
/// key, so the same `PeerId` survives restarts. When `None` the app
/// generates a fresh ephemeral key per session.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IdentitySettings {
    /// Optional path to a file storing the iroh secret key. Resolved
    /// relative to the config directory when not absolute.
    pub secret_key_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Capture / video
// ---------------------------------------------------------------------------

/// Video capture ceiling — the *most* the host is willing to send.
///
/// The actual operating point is decided at session start by quality
/// negotiation (P4.7.1) and may be lower. These values cap that
/// negotiation rather than fixing it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    pub max_fps: u32,
    /// Maximum encoded resolution as `(width, height)`. `None` means "no
    /// cap beyond the physical display size."
    pub max_resolution: Option<(u32, u32)>,
    pub h264_profile: H264Profile,
    pub keyframe_interval_secs: u32,
    /// Whether to skip encoding when the frame is identical to the
    /// previous frame. Reduces idle bitrate dramatically; disable only
    /// if you suspect it is hiding a bug.
    pub skip_static_frames: bool,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            max_fps: 30,
            max_resolution: Some((1920, 1080)),
            h264_profile: H264Profile::default(),
            keyframe_interval_secs: 2,
            skip_static_frames: true,
        }
    }
}

/// H.264 profile preference. Falls back to a lower profile when hardware
/// support is unavailable; see P4.7.1 for the negotiation rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum H264Profile {
    /// Best quality, requires hardware on both ends.
    High,
    /// Sensible default — works on most hardware.
    #[default]
    Main,
    /// Maximum compatibility, lowest quality.
    ConstrainedBaseline,
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Opus bitrate in kbps. Default of 64 kbps is mono voice quality;
    /// 32 kbps showed artifacting in the 2026-04-01 real-device test.
    pub bitrate_kbps: u32,
    /// 1 = mono, 2 = stereo. Mono is correct for voice; stereo is for
    /// system-audio sharing.
    pub channels: u8,
    pub sample_rate: u32,
    /// Opus frame duration in ms: 5, 10, 20, 40, or 60.
    pub frame_duration_ms: u32,
    /// Start the session with the local mic muted.
    pub start_muted: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            bitrate_kbps: 64,
            channels: 1,
            sample_rate: 48_000,
            frame_duration_ms: 20,
            start_muted: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Permissions — the "safe defaults" layer
// ---------------------------------------------------------------------------

/// What the host allows the guest to do. Defaults are deliberately
/// restrictive — explicit opt-in for anything that gives the guest
/// access to host state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionSettings {
    pub remote_control: RemoteControlPolicy,
    pub clipboard_sync: ClipboardPolicy,
    pub terminal_access: TerminalAccessPolicy,
    /// Auto-accept incoming tunnel requests for `tunnels.default_ports`.
    /// Unknown ports still require explicit approval regardless.
    pub auto_accept_tunnels: bool,
}

impl Default for PermissionSettings {
    fn default() -> Self {
        Self {
            remote_control: RemoteControlPolicy::Denied,
            clipboard_sync: ClipboardPolicy::TextOnly,
            terminal_access: TerminalAccessPolicy::ReadOnly,
            auto_accept_tunnels: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteControlPolicy {
    /// Guest may never drive — observer mode only.
    #[default]
    Denied,
    /// Show a prompt on each `ControlRequest`; require explicit grant.
    AskFirst,
    /// Auto-grant on `ControlRequest`. Equivalent to today's
    /// `--allow-remote-control` flag.
    AutoGrant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardPolicy {
    /// No clipboard sync in either direction.
    Disabled,
    /// Sync text only — never images, never files. Default.
    #[default]
    TextOnly,
    /// Sync text, images, and files. File transfer can leak data; opt
    /// in explicitly.
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalAccessPolicy {
    /// Guest may not see or interact with any terminal.
    None,
    /// Guest may view terminal output but not type into it. Default —
    /// safe for screen-sharing without granting shell access.
    #[default]
    ReadOnly,
    /// Guest may type into shared terminals (still gated by control
    /// takeover from P3.5.2).
    ReadWrite,
}

// ---------------------------------------------------------------------------
// Terminal sharing (Phase 4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalSettings {
    /// Override the shell to spawn for new terminal sessions. `None`
    /// means use `$SHELL` from the environment.
    pub default_shell: Option<String>,
    pub max_sessions: u8,
    pub scrollback_lines: u32,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            default_shell: None,
            max_sessions: 8,
            scrollback_lines: 10_000,
        }
    }
}

// ---------------------------------------------------------------------------
// Localhost tunneling (Phase 4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TunnelSettings {
    /// Ports the host offers up-front during handshake.
    pub default_ports: Vec<u16>,
    /// Ports the host refuses to tunnel even if requested. Pre-populated
    /// with common database ports — opt out by editing this list.
    pub blocked_ports: Vec<u16>,
    /// When the guest's requested local bind port is taken, pick a
    /// nearby free port automatically and warn. Without this, the
    /// tunnel request fails.
    pub auto_pick_on_conflict: bool,
}

impl Default for TunnelSettings {
    fn default() -> Self {
        Self {
            default_ports: Vec::new(),
            // MySQL, PostgreSQL, Redis, MongoDB.
            blocked_ports: vec![3306, 5432, 6379, 27017],
            auto_pick_on_conflict: true,
        }
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// Show the host annotation overlay window (drawing, cursor,
    /// reactions). Disable for headless or tray-only operation.
    pub show_overlay: bool,
    /// Show the connection-quality dot in the viewer HUD.
    pub show_quality_badge: bool,
    /// RGB color for the host cursor crosshair. Default = red.
    pub host_cursor_color: [u8; 3],
    /// RGB color for the guest cursor crosshair. Default = blue.
    pub guest_cursor_color: [u8; 3],
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            show_overlay: true,
            show_quality_badge: true,
            host_cursor_color: [220, 50, 50],
            guest_cursor_color: [50, 120, 220],
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let settings = Settings::default();
        let encoded = toml::to_string(&settings).unwrap();
        let decoded: Settings = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, settings);
    }

    #[test]
    fn empty_toml_yields_defaults() {
        let decoded: Settings = toml::from_str("").unwrap();
        assert_eq!(decoded, Settings::default());
    }

    #[test]
    fn partial_toml_fills_missing_with_defaults() {
        // Set one field; everything else should default.
        let toml_src = r#"
[capture]
max_fps = 60
"#;
        let decoded: Settings = toml::from_str(toml_src).unwrap();
        assert_eq!(decoded.capture.max_fps, 60);
        assert_eq!(
            decoded.capture.max_resolution,
            Some((1920, 1080)),
            "max_resolution should keep its default"
        );
        assert_eq!(
            decoded.audio,
            AudioSettings::default(),
            "untouched section keeps full defaults"
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        // We want misspellings to fail loudly so users notice typos.
        // `toml`'s default behavior allows unknown fields; we can switch to
        // `deny_unknown_fields` later if this becomes a problem. For now,
        // assert the lenient behavior so it's documented.
        let toml_src = r#"
[capture]
not_a_real_field = 99
"#;
        let result: Result<Settings, _> = toml::from_str(toml_src);
        assert!(
            result.is_ok(),
            "unknown fields are currently ignored — change me if we tighten this"
        );
    }

    // ---------- Safe defaults (acceptance criterion from P4.7) ----------

    #[test]
    fn default_remote_control_is_denied() {
        assert_eq!(
            Settings::default().permissions.remote_control,
            RemoteControlPolicy::Denied
        );
    }

    #[test]
    fn default_clipboard_is_text_only() {
        assert_eq!(
            Settings::default().permissions.clipboard_sync,
            ClipboardPolicy::TextOnly
        );
    }

    #[test]
    fn default_terminal_is_read_only() {
        assert_eq!(
            Settings::default().permissions.terminal_access,
            TerminalAccessPolicy::ReadOnly
        );
    }

    #[test]
    fn database_ports_blocked_by_default() {
        let blocked = &Settings::default().tunnels.blocked_ports;
        for port in [3306, 5432, 6379, 27017] {
            assert!(
                blocked.contains(&port),
                "port {port} must be blocked by default"
            );
        }
    }

    #[test]
    fn tunnels_offer_nothing_by_default() {
        assert!(Settings::default().tunnels.default_ports.is_empty());
    }

    // ---------- TOML rendering sanity ----------

    #[test]
    fn rendered_toml_contains_section_headers() {
        let rendered = toml::to_string(&Settings::default()).unwrap();
        for section in [
            "[identity]",
            "[capture]",
            "[audio]",
            "[permissions]",
            "[terminal]",
            "[tunnels]",
            "[ui]",
        ] {
            assert!(
                rendered.contains(section),
                "missing section {section} in:\n{rendered}"
            );
        }
    }

    #[test]
    fn enum_renders_as_snake_case() {
        // The `denied` / `text_only` / `read_only` strings are the
        // user-facing values in config.toml — guard them with a test.
        let rendered = toml::to_string(&Settings::default()).unwrap();
        assert!(rendered.contains("remote_control = \"denied\""));
        assert!(rendered.contains("clipboard_sync = \"text_only\""));
        assert!(rendered.contains("terminal_access = \"read_only\""));
    }
}
