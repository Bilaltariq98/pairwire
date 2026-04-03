//! CLI argument parsing for pairwire.
//!
//! Commands:
//!   pairwire host [--fps N] [--width W] [--height H]
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

#[derive(Debug)]
pub struct HostArgs {
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    /// Whether to auto-grant remote control permission to the guest.
    pub allow_remote_control: bool,
}

impl Default for HostArgs {
    fn default() -> Self {
        Self {
            fps: 30,
            width: 1920,
            height: 1080,
            allow_remote_control: false,
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
                    "--width" => {
                        i += 1;
                        a.width = parse_u32(args, i, "--width")?;
                    }
                    "--height" => {
                        i += 1;
                        a.height = parse_u32(args, i, "--height")?;
                    }
                    "--allow-remote-control" => {
                        a.allow_remote_control = true;
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
        assert_eq!(a.width, 1920);
    }

    #[test]
    fn parse_host_with_flags() {
        let cmd = parse(&["host", "--fps", "60", "--width", "2560", "--height", "1440"]).unwrap();
        let Command::Host(a) = cmd else {
            panic!("expected Host")
        };
        assert_eq!(a.fps, 60);
        assert_eq!(a.width, 2560);
        assert_eq!(a.height, 1440);
        assert!(!a.allow_remote_control);
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
