//! pairwire — P2P screen sharing and pair programming.
//!
//! Usage:
//!   pairwire host              Start a session and print an invite link
//!   pairwire join <invite>     Join a session using an invite link
//!
//! Subcommands and full CLI are implemented in P1.10.

use anyhow::Result;
use tracing::info;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("pairwire=debug".parse()?),
        )
        .init();

    info!(version = env!("CARGO_PKG_VERSION"), "pairwire starting");

    // CLI dispatch implemented in P1.10.
    // For now, print usage and exit cleanly.
    eprintln!("pairwire v{}", env!("CARGO_PKG_VERSION"));
    eprintln!("Usage:");
    eprintln!("  pairwire host            Start a session");
    eprintln!("  pairwire join <invite>   Join a session");

    Ok(())
}
