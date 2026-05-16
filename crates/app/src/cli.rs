//! CLI argument parsing for pairwire.
//!
//! Commands:
//!   pairwire host [--fps N] [--resolution native|1080p|720p]
//!   pairwire join <invite-url> [--no-gui]
//!
//! Argument resolution is a three-layer merge:
//!
//! ```text
//! Settings::default()  ─┐
//! config.toml           ┼──►  HostArgs::from_settings(&merged)
//! CLI flags            ─┘     └─►  overlaid by parse_from_args()
//! ```
//!
//! [`load_settings_with_fallback`] loads the on-disk config; bad files
//! warn-and-default rather than abort startup.

use pairwire_core::{RemoteControlPolicy, Settings};
use std::path::Path;
use std::str::FromStr;

/// Top-level CLI command.
#[derive(Debug)]
pub enum Command {
    /// Start a hosting session and print an invite link.
    Host(HostArgs),
    /// Join a session using an invite link.
    Join(JoinArgs),
    /// Inspect or modify the persisted user settings (`config.toml`).
    Config(ConfigArgs),
}

/// Subcommand under `pairwire config`.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigArgs {
    /// Print the resolved path to `config.toml`.
    Path,
    /// Read one dotted-key value (e.g. `capture.max_fps`).
    Get { key: String },
    /// Set one dotted-key value, preserving comments in the file.
    Set { key: String, value: String },
    /// Open `config.toml` in `$EDITOR` (falls back to `vi`).
    Edit,
}

/// Resolution preset for the capture pipeline.
///
/// `Native` captures at the display's physical pixel resolution (correct for Retina).
/// `P1080` and `P720` cap the encode resolution regardless of display size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResolutionPreset {
    /// Detect the physical display resolution at startup and use it directly.
    Native,
    /// Cap at 1920×1080 — good default for most network conditions.
    #[default]
    P1080,
    /// Cap at 1280×720 — for low-bandwidth or slow connections.
    P720,
}

impl ResolutionPreset {
    /// Resolve to concrete (width, height) given the detected physical display size.
    ///
    /// Both dimensions are rounded down to the nearest even integer because H.264
    /// requires even width/height (CaptureConfig::validate enforces this).
    pub fn resolve(self, physical: (u32, u32)) -> (u32, u32) {
        let (w, h) = match self {
            ResolutionPreset::Native => physical,
            ResolutionPreset::P1080 => (physical.0.min(1920), physical.1.min(1080)),
            ResolutionPreset::P720 => (physical.0.min(1280), physical.1.min(720)),
        };
        (w & !1, h & !1)
    }
}

impl FromStr for ResolutionPreset {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "native" => Ok(Self::Native),
            "1080p" => Ok(Self::P1080),
            "720p" => Ok(Self::P720),
            other => {
                anyhow::bail!("unknown resolution preset '{other}' — use native, 1080p, or 720p")
            }
        }
    }
}

#[derive(Debug)]
pub struct HostArgs {
    pub fps: u32,
    pub resolution: ResolutionPreset,
    /// When set, automatically grants `FullControl` the moment the guest sends a
    /// `ControlRequest`. When unset, the host must click "Allow Remote Control" in
    /// the tray menu to grant.
    pub allow_remote_control: bool,
    /// When set, skip the host annotation overlay window even when the `gui`
    /// feature is compiled in. Useful for headless deployments, CI, or when the
    /// host prefers a pure tray-icon workflow.
    pub no_overlay: bool,
}

impl Default for HostArgs {
    fn default() -> Self {
        // Keep this in sync with `Settings::default()` — covered by
        // `from_settings_default_matches_hostargs_default` test.
        Self {
            fps: 30,
            resolution: ResolutionPreset::default(),
            allow_remote_control: false,
            no_overlay: false,
        }
    }
}

impl HostArgs {
    /// Derive defaults from a loaded `Settings`. The result is what
    /// CLI-flag parsing then layers on top of.
    ///
    /// `resolution` is intentionally **not** sourced from
    /// `settings.capture.max_resolution` yet — the preset enum
    /// (Native/1080p/720p) doesn't map cleanly to arbitrary dims, and
    /// P4.7.1 (Intelligent Quality Negotiation) is going to rework the
    /// resolution-selection story anyway. For now, resolution stays
    /// CLI-driven.
    pub fn from_settings(s: &Settings) -> Self {
        let allow_remote_control = match s.permissions.remote_control {
            RemoteControlPolicy::AutoGrant => true,
            RemoteControlPolicy::Denied => false,
            RemoteControlPolicy::AskFirst => {
                // TODO(P3.5.2 follow-up): wire to a real "approve?"
                // prompt. Until that exists, AskFirst is indistinguishable
                // from Denied — make the gap visible in the logs.
                tracing::warn!(
                    "permissions.remote_control = 'ask_first' has no prompt UI yet — \
                     treating as 'denied' until that lands"
                );
                false
            }
        };
        Self {
            fps: s.capture.max_fps,
            resolution: ResolutionPreset::default(),
            allow_remote_control,
            no_overlay: !s.ui.show_overlay,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JoinArgs {
    pub invite_url: String,
    /// Skip the GUI and just print decoded frame stats (for testing).
    pub no_gui: bool,
}

/// Parse `std::env::args()` into a `Command`, merging on-disk settings
/// underneath the CLI flags.
///
/// Layer order (later layers win): `Settings::default()` →
/// `config.toml` → CLI flags. The config file is created with safe
/// defaults on first run; load failures degrade to defaults rather
/// than abort startup.
pub fn parse_args() -> anyhow::Result<Command> {
    let settings = load_settings_with_fallback();
    let args: Vec<String> = std::env::args().skip(1).collect();
    parse_from_args(&args, &settings)
}

/// Load `Settings` from the OS-default config path.
///
/// If the path cannot be determined or the file is malformed, logs a
/// warning and returns `Settings::default()` — pairwire should never
/// be unable to start because of a config file.
pub fn load_settings_with_fallback() -> Settings {
    let path = match pairwire_core::default_config_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "could not determine config path; using defaults");
            return Settings::default();
        }
    };
    let settings = load_settings_at(&path);
    tracing::info!(path = %path.display(), "loaded settings");
    settings
}

/// Testable inner: load from an explicit path with the same
/// warn-and-default fallback as `load_settings_with_fallback`.
fn load_settings_at(path: &Path) -> Settings {
    match pairwire_core::load_or_init(path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "failed to load config; using defaults"
            );
            Settings::default()
        }
    }
}

/// Parse a pre-split argument list (without the program name) into a `Command`.
fn parse_from_args(args: &[String], settings: &Settings) -> anyhow::Result<Command> {
    match args.first().map(std::string::String::as_str) {
        Some("host") => {
            let mut a = HostArgs::from_settings(settings);
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--fps" => {
                        i += 1;
                        a.fps = parse_u32(args, i, "--fps")?;
                    }
                    "--resolution" => {
                        i += 1;
                        let val = args
                            .get(i)
                            .ok_or_else(|| anyhow::anyhow!("--resolution requires a value"))?;
                        a.resolution = val.parse()?;
                    }
                    "--allow-remote-control" => {
                        a.allow_remote_control = true;
                    }
                    "--no-overlay" => {
                        a.no_overlay = true;
                    }
                    flag => anyhow::bail!("unknown flag: {flag}"),
                }
                i += 1;
            }
            Ok(Command::Host(a))
        }
        Some("join") => {
            if args.len() < 2 {
                anyhow::bail!("usage: pairwire join <invite-url> [--no-gui]");
            }
            let invite_url = args[1].clone();
            let no_gui = args.iter().any(|a| a == "--no-gui");
            Ok(Command::Join(JoinArgs { invite_url, no_gui }))
        }
        Some("config") => Ok(Command::Config(parse_config_args(&args[1..])?)),
        Some(cmd) => {
            anyhow::bail!(
                "unknown command: {cmd}\n\
                 Usage:\n\
                 \x20 pairwire host [--fps N] [--resolution native|1080p|720p]\n\
                 \x20 pairwire join <invite-url> [--no-gui]\n\
                 \x20 pairwire config {{path | get <key> | set <key> <value> | edit}}"
            )
        }
        None => anyhow::bail!(
            "Usage:\n\
             \x20 pairwire host\n\
             \x20 pairwire join <invite>\n\
             \x20 pairwire config {{path | get <key> | set <key> <value> | edit}}"
        ),
    }
}

/// Parse the tail after `config`: `path`, `get <key>`, `set <key> <value>`, or `edit`.
fn parse_config_args(rest: &[String]) -> anyhow::Result<ConfigArgs> {
    match rest.first().map(std::string::String::as_str) {
        Some("path") => Ok(ConfigArgs::Path),
        Some("edit") => Ok(ConfigArgs::Edit),
        Some("get") => {
            let key = rest
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("usage: pairwire config get <key>"))?
                .clone();
            Ok(ConfigArgs::Get { key })
        }
        Some("set") => {
            let key = rest
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("usage: pairwire config set <key> <value>"))?
                .clone();
            let value = rest
                .get(2)
                .ok_or_else(|| anyhow::anyhow!("usage: pairwire config set <key> <value>"))?
                .clone();
            Ok(ConfigArgs::Set { key, value })
        }
        Some(other) => {
            anyhow::bail!("unknown config subcommand: {other} (try: path, get, set, edit)")
        }
        None => {
            anyhow::bail!("usage: pairwire config {{path | get <key> | set <key> <value> | edit}}")
        }
    }
}

fn parse_u32(args: &[String], i: usize, flag: &str) -> anyhow::Result<u32> {
    args.get(i)
        .ok_or_else(|| anyhow::anyhow!("{flag} requires a value"))
        .and_then(|v| u32::from_str(v).map_err(|_| anyhow::anyhow!("{flag} must be a number")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<Command> {
        parse_with(args, &Settings::default())
    }

    fn parse_with(args: &[&str], settings: &Settings) -> anyhow::Result<Command> {
        let args: Vec<String> = args.iter().map(|s| String::from(*s)).collect();
        parse_from_args(&args, settings)
    }

    #[test]
    fn parse_host_defaults() {
        let cmd = parse(&["host"]).unwrap();
        let Command::Host(a) = cmd else {
            panic!("expected Host")
        };
        assert_eq!(a.fps, 30);
        assert_eq!(a.resolution, ResolutionPreset::P1080);
    }

    #[test]
    fn parse_host_with_flags() {
        let cmd = parse(&["host", "--fps", "60", "--resolution", "native"]).unwrap();
        let Command::Host(a) = cmd else {
            panic!("expected Host")
        };
        assert_eq!(a.fps, 60);
        assert_eq!(a.resolution, ResolutionPreset::Native);
        assert!(!a.allow_remote_control);
    }

    #[test]
    fn parse_host_resolution_720p() {
        let cmd = parse(&["host", "--resolution", "720p"]).unwrap();
        let Command::Host(a) = cmd else {
            panic!("expected Host")
        };
        assert_eq!(a.resolution, ResolutionPreset::P720);
    }

    #[test]
    fn resolution_preset_resolve_native() {
        let physical = (3024, 1964);
        assert_eq!(ResolutionPreset::Native.resolve(physical), (3024, 1964));
    }

    #[test]
    fn resolution_preset_resolve_1080p_caps_retina() {
        let physical = (3024, 1964);
        assert_eq!(ResolutionPreset::P1080.resolve(physical), (1920, 1080));
    }

    #[test]
    fn resolution_preset_resolve_1080p_passthrough_small() {
        // A 1280×800 display should not be upscaled.
        let physical = (1280, 800);
        assert_eq!(ResolutionPreset::P1080.resolve(physical), (1280, 800));
    }

    #[test]
    fn resolution_preset_resolve_rounds_odd_to_even() {
        // H.264 requires even width/height; odd physical dims must be rounded down.
        let physical = (2056, 1329);
        assert_eq!(ResolutionPreset::Native.resolve(physical), (2056, 1328));
    }

    #[test]
    fn resolution_preset_resolve_720p() {
        let physical = (1920, 1080);
        assert_eq!(ResolutionPreset::P720.resolve(physical), (1280, 720));
    }

    #[test]
    fn parse_host_allow_remote_control() {
        let cmd = parse(&["host", "--allow-remote-control"]).unwrap();
        let Command::Host(a) = cmd else {
            panic!("expected Host")
        };
        assert!(a.allow_remote_control);
    }

    #[test]
    fn parse_host_no_overlay() {
        let cmd = parse(&["host", "--no-overlay"]).unwrap();
        let Command::Host(a) = cmd else {
            panic!("expected Host")
        };
        assert!(a.no_overlay);
        assert!(!a.allow_remote_control);
    }

    #[test]
    fn parse_host_defaults_no_overlay_false() {
        let cmd = parse(&["host"]).unwrap();
        let Command::Host(a) = cmd else {
            panic!("expected Host")
        };
        assert!(!a.no_overlay);
    }

    #[test]
    fn parse_join() {
        let cmd = parse(&[
            "join",
            "pairwire://abc?token=00000000000000000000000000000000",
        ])
        .unwrap();
        let Command::Join(a) = cmd else {
            panic!("expected Join")
        };
        assert!(a.invite_url.starts_with("pairwire://"));
        assert!(!a.no_gui);
    }

    #[test]
    fn parse_join_no_gui() {
        let cmd = parse(&[
            "join",
            "pairwire://abc?token=00000000000000000000000000000000",
            "--no-gui",
        ])
        .unwrap();
        let Command::Join(a) = cmd else {
            panic!("expected Join")
        };
        assert!(a.no_gui);
    }

    #[test]
    fn parse_unknown_command_errors() {
        assert!(parse(&["serve"]).is_err());
    }

    #[test]
    fn parse_no_args_errors() {
        assert!(parse(&[]).is_err());
    }

    // ───────────────────── config subcommand parsing ─────────────────────

    #[test]
    fn parse_config_path() {
        let Command::Config(c) = parse(&["config", "path"]).unwrap() else {
            panic!("expected Config")
        };
        assert_eq!(c, ConfigArgs::Path);
    }

    #[test]
    fn parse_config_edit() {
        let Command::Config(c) = parse(&["config", "edit"]).unwrap() else {
            panic!("expected Config")
        };
        assert_eq!(c, ConfigArgs::Edit);
    }

    #[test]
    fn parse_config_get() {
        let Command::Config(c) = parse(&["config", "get", "capture.max_fps"]).unwrap() else {
            panic!("expected Config")
        };
        assert_eq!(
            c,
            ConfigArgs::Get {
                key: "capture.max_fps".into(),
            }
        );
    }

    #[test]
    fn parse_config_set() {
        let Command::Config(c) = parse(&["config", "set", "capture.max_fps", "60"]).unwrap() else {
            panic!("expected Config")
        };
        assert_eq!(
            c,
            ConfigArgs::Set {
                key: "capture.max_fps".into(),
                value: "60".into(),
            }
        );
    }

    #[test]
    fn parse_config_get_missing_key_errors() {
        assert!(parse(&["config", "get"]).is_err());
    }

    #[test]
    fn parse_config_set_missing_value_errors() {
        assert!(parse(&["config", "set", "capture.max_fps"]).is_err());
    }

    #[test]
    fn parse_config_unknown_sub_errors() {
        assert!(parse(&["config", "spelunk"]).is_err());
    }

    #[test]
    fn parse_config_no_sub_errors() {
        assert!(parse(&["config"]).is_err());
    }

    // ───────────────────── Settings merge ─────────────────────

    #[test]
    fn from_settings_default_matches_hostargs_default() {
        // Baseline invariant: feeding default Settings through
        // `from_settings` must produce the same shape as the current
        // hardcoded `HostArgs::default()`. If this breaks, one of the
        // two has drifted and needs reconciling.
        let from_default = HostArgs::from_settings(&Settings::default());
        let hardcoded = HostArgs::default();
        assert_eq!(from_default.fps, hardcoded.fps);
        assert_eq!(from_default.resolution, hardcoded.resolution);
        assert_eq!(
            from_default.allow_remote_control,
            hardcoded.allow_remote_control
        );
        assert_eq!(from_default.no_overlay, hardcoded.no_overlay);
    }

    #[test]
    fn settings_fps_inherited_when_no_cli_flag() {
        let mut settings = Settings::default();
        settings.capture.max_fps = 60;
        let Command::Host(a) = parse_with(&["host"], &settings).unwrap() else {
            panic!("expected Host")
        };
        assert_eq!(a.fps, 60, "settings.capture.max_fps should flow through");
    }

    #[test]
    fn cli_fps_overrides_settings_fps() {
        let mut settings = Settings::default();
        settings.capture.max_fps = 60;
        let Command::Host(a) = parse_with(&["host", "--fps", "24"], &settings).unwrap() else {
            panic!("expected Host")
        };
        assert_eq!(a.fps, 24, "CLI --fps must win over settings");
    }

    #[test]
    fn settings_auto_grant_enables_allow_remote_control() {
        let mut settings = Settings::default();
        settings.permissions.remote_control = RemoteControlPolicy::AutoGrant;
        let Command::Host(a) = parse_with(&["host"], &settings).unwrap() else {
            panic!("expected Host")
        };
        assert!(a.allow_remote_control);
    }

    #[test]
    fn settings_ask_first_treated_as_denied() {
        // Until the prompt UI lands, AskFirst must not silently behave
        // like AutoGrant. Guards against accidental privilege grant.
        let mut settings = Settings::default();
        settings.permissions.remote_control = RemoteControlPolicy::AskFirst;
        let Command::Host(a) = parse_with(&["host"], &settings).unwrap() else {
            panic!("expected Host")
        };
        assert!(!a.allow_remote_control);
    }

    #[test]
    fn cli_allow_remote_control_overrides_denied_settings() {
        // settings explicitly denies; CLI flag should still flip it on
        // (operator-on-the-machine intent beats persisted config).
        let settings = Settings::default(); // remote_control = Denied
        let Command::Host(a) = parse_with(&["host", "--allow-remote-control"], &settings).unwrap()
        else {
            panic!("expected Host")
        };
        assert!(a.allow_remote_control);
    }

    #[test]
    fn settings_show_overlay_false_yields_no_overlay_true() {
        let mut settings = Settings::default();
        settings.ui.show_overlay = false;
        let Command::Host(a) = parse_with(&["host"], &settings).unwrap() else {
            panic!("expected Host")
        };
        assert!(a.no_overlay);
    }

    #[test]
    fn cli_no_overlay_overrides_show_overlay_true_settings() {
        let settings = Settings::default(); // show_overlay = true
        let Command::Host(a) = parse_with(&["host", "--no-overlay"], &settings).unwrap() else {
            panic!("expected Host")
        };
        assert!(a.no_overlay);
    }

    // ───────────────────── load_settings_at fallback ─────────────────────

    #[test]
    fn load_settings_at_creates_file_on_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(!path.exists());
        let s = load_settings_at(&path);
        assert_eq!(s, Settings::default());
        assert!(path.exists(), "first-run template should be written");
    }

    #[test]
    fn load_settings_at_malformed_file_returns_defaults() {
        // Acceptance criterion: a busted config must not brick startup.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is = = not toml [[[").unwrap();
        let s = load_settings_at(&path);
        assert_eq!(
            s,
            Settings::default(),
            "malformed config should fall back to defaults"
        );
    }
}
