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
    pub fn resolve(self, physical: (u32, u32)) -> (u32, u32) {
        match self {
            ResolutionPreset::Native => physical,
            ResolutionPreset::P1080 => (physical.0.min(1920), physical.1.min(1080)),
            ResolutionPreset::P720 => (physical.0.min(1280), physical.1.min(720)),
        }
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
    /// When set (and `gui` feature is on), use the experimental transparent
    /// fullscreen overlay instead of the mirror-window overlay. The transparent
    /// overlay sits over the host's desktop and draws strokes directly on top;
    /// the host can use Cmd+Shift+D to enter/exit drawing mode, with click-through
    /// enabled when not drawing.
    pub transparent_overlay: bool,
}

impl Default for HostArgs {
    fn default() -> Self {
        Self {
            fps: 30,
            resolution: ResolutionPreset::default(),
            allow_remote_control: false,
            no_overlay: false,
            transparent_overlay: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JoinArgs {
    pub invite_url: String,
    /// Skip the GUI and just print decoded frame stats (for testing).
    pub no_gui: bool,
}

/// Parse `std::env::args()` into a `Command`.
pub fn parse_args() -> anyhow::Result<Command> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    parse_from_args(&args)
}

/// Parse a pre-split argument list (without the program name) into a `Command`.
fn parse_from_args(args: &[String]) -> anyhow::Result<Command> {
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
                    "--transparent-overlay" => {
                        a.transparent_overlay = true;
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
        Some(cmd) => {
            anyhow::bail!("unknown command: {cmd}\nUsage: pairwire host | pairwire join <invite>")
        }
        None => anyhow::bail!("Usage: pairwire host | pairwire join <invite>"),
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
}
