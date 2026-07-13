//! CLI argument parsing for pairwire.
//!
//! Commands:
//!   pairwire host [--fps N] [--resolution native|1080p|720p]
//!   pairwire join <invite-url> [--no-gui]

use std::str::FromStr;

/// Top-level CLI command.
#[derive(Debug)]
pub enum Command {
    /// Start a hosting session and print an invite link.
    Host(HostArgs),
    /// Join a session using an invite link.
    Join(JoinArgs),
    /// No subcommand given (double-clicked app): show the native Host/Join chooser.
    Launcher,
    /// Print the version string and exit.
    Version,
    /// Print usage help and exit.
    Help,
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
    /// When set, the host's microphone starts muted. The capture thread still
    /// runs and emits silence frames; mute can be toggled at runtime via the
    /// tray menu.
    pub mute: bool,
}

impl Default for HostArgs {
    fn default() -> Self {
        Self {
            fps: 30,
            resolution: ResolutionPreset::default(),
            allow_remote_control: false,
            no_overlay: false,
            mute: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JoinArgs {
    pub invite_url: String,
    /// Skip the GUI and just print decoded frame stats (for testing).
    pub no_gui: bool,
    /// When set, the guest's microphone starts muted. Toggle at runtime via
    /// the viewer toolbar button or Cmd/Ctrl+Shift+M.
    pub mute: bool,
}

/// Usage help text printed by `--help` / `-h`.
pub const HELP_TEXT: &str = "\
pairwire — P2P screen sharing and pair programming

USAGE:
    pairwire host [OPTIONS]
    pairwire join <invite-url> [OPTIONS]
    pairwire --version
    pairwire --help

COMMANDS:
    host    Start a sharing session and print an invite link
    join    Join a session using an invite link

OPTIONS (host):
    --fps <N>                   Capture frame rate (default: 30)
    --resolution <preset>       native | 1080p | 720p (default: 1080p)
    --allow-remote-control      Auto-grant remote control to guests
    --no-overlay                Skip the annotation overlay window

OPTIONS (join):
    --no-gui                    Run headless (for testing)

GLOBAL OPTIONS:
    --version, -V               Print version and exit
    --help, -h                  Print this help and exit
";

/// Parse `std::env::args()` into a `Command`.
pub fn parse_args() -> anyhow::Result<Command> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    parse_from_args(&args)
}

/// Parse a pre-split argument list (without the program name) into a `Command`.
fn parse_from_args(args: &[String]) -> anyhow::Result<Command> {
    // Handle global flags before subcommand dispatch.
    if let Some(first) = args.first() {
        match first.as_str() {
            "--version" | "-V" => return Ok(Command::Version),
            "--help" | "-h" => return Ok(Command::Help),
            _ => {}
        }
    }
    match args.first().map(std::string::String::as_str) {
        Some("host") => {
            let mut a = HostArgs::default();
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
                    "--mute" => {
                        a.mute = true;
                    }
                    flag => anyhow::bail!("unknown flag: {flag}"),
                }
                i += 1;
            }
            Ok(Command::Host(a))
        }
        Some("join") => {
            if args.len() < 2 {
                anyhow::bail!("usage: pairwire join <invite-url> [--no-gui] [--mute]");
            }
            let invite_url = args[1].clone();
            validate_invite(&invite_url)?;
            let no_gui = args.iter().any(|a| a == "--no-gui");
            let mute = args.iter().any(|a| a == "--mute");
            Ok(Command::Join(JoinArgs {
                invite_url,
                no_gui,
                mute,
            }))
        }
        Some(cmd) => {
            anyhow::bail!("unknown command: {cmd}\nUsage: pairwire host | pairwire join <invite>")
        }
        None => Ok(Command::Launcher),
    }
}

fn parse_u32(args: &[String], i: usize, flag: &str) -> anyhow::Result<u32> {
    args.get(i)
        .ok_or_else(|| anyhow::anyhow!("{flag} requires a value"))
        .and_then(|v| u32::from_str(v).map_err(|_| anyhow::anyhow!("{flag} must be a number")))
}

/// Validate an invite URL at the boundary. Pairwire invites are `pairwire://...`.
pub fn validate_invite(url: &str) -> anyhow::Result<()> {
    let url = url.trim();
    if url.is_empty() {
        anyhow::bail!("invite link is empty");
    }
    if !url.starts_with("pairwire://") {
        anyhow::bail!("invite link must start with pairwire://");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<Command> {
        let args: Vec<String> = args.iter().map(|s| String::from(*s)).collect();
        parse_from_args(&args)
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
    fn parse_no_args_returns_launcher() {
        let cmd = parse(&[]).unwrap();
        assert!(matches!(cmd, Command::Launcher));
    }

    #[test]
    fn validate_invite_accepts_pairwire_url() {
        assert!(validate_invite("pairwire://abc?token=0000000000000000000000000000000000").is_ok());
    }

    #[test]
    fn validate_invite_rejects_empty_and_non_pairwire() {
        assert!(validate_invite("").is_err());
        assert!(validate_invite("https://example.com").is_err());
    }

    #[test]
    fn parse_join_rejects_non_pairwire_url() {
        // A plain HTTPS URL must be rejected at the parse boundary.
        assert!(parse(&["join", "https://example.com/invite"]).is_err());
    }

    #[test]
    fn parse_join_rejects_flag_as_url() {
        // `pairwire join --no-gui` would previously store "--no-gui" as the
        // invite URL and only fail much later. With validate_invite wired in,
        // this is caught immediately at the CLI boundary.
        assert!(parse(&["join", "--no-gui"]).is_err());
    }

    #[test]
    fn parse_join_rejects_empty_url() {
        assert!(parse(&["join", ""]).is_err());
    }

    #[test]
    fn parse_version_flag() {
        let cmd = parse(&["--version"]).unwrap();
        let Command::Version = cmd else {
            panic!("expected Version command");
        };
    }

    #[test]
    fn parse_version_short() {
        let cmd = parse(&["-V"]).unwrap();
        let Command::Version = cmd else {
            panic!("expected Version command");
        };
    }

    #[test]
    fn parse_help_flag() {
        let cmd = parse(&["--help"]).unwrap();
        let Command::Help = cmd else {
            panic!("expected Help command");
        };
    }

    #[test]
    fn parse_help_short() {
        let cmd = parse(&["-h"]).unwrap();
        let Command::Help = cmd else {
            panic!("expected Help command");
        };
    }

    // ─────────────────────────── --mute ───────────────────────────

    #[test]
    fn parse_host_default_unmuted() {
        let cmd = parse(&["host"]).unwrap();
        let Command::Host(a) = cmd else {
            panic!("expected Host")
        };
        assert!(!a.mute);
    }

    #[test]
    fn parse_host_mute() {
        let cmd = parse(&["host", "--mute"]).unwrap();
        let Command::Host(a) = cmd else {
            panic!("expected Host")
        };
        assert!(a.mute);
    }

    #[test]
    fn parse_join_default_unmuted() {
        let cmd = parse(&[
            "join",
            "pairwire://abc?token=00000000000000000000000000000000",
        ])
        .unwrap();
        let Command::Join(a) = cmd else {
            panic!("expected Join")
        };
        assert!(!a.mute);
    }

    #[test]
    fn parse_join_mute() {
        let cmd = parse(&[
            "join",
            "pairwire://abc?token=00000000000000000000000000000000",
            "--mute",
        ])
        .unwrap();
        let Command::Join(a) = cmd else {
            panic!("expected Join")
        };
        assert!(a.mute);
    }
}
