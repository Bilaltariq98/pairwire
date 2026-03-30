//! pairwire — P2P screen sharing and pair programming.
//!
//! Usage:
//!   pairwire host [--fps N] [--width W] [--height H]
//!   pairwire join <invite-url> [--no-gui]

mod cli;
mod session;
mod viewer;

use anyhow::Result;
use tracing::info;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("pairwire=info".parse()?)
                .add_directive("iroh=warn".parse()?),
        )
        .init();

    info!(version = env!("CARGO_PKG_VERSION"), "pairwire starting");

    let cmd = cli::parse_args()?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    match cmd {
        cli::Command::Host(args) => {
            rt.block_on(session::run_host(args))?;
        }
        cli::Command::Join(args) => {
            let (frame_tx, frame_rx) =
                tokio::sync::watch::channel::<Option<pairwire_capture::DecodedFrame>>(None);
            let (cursor_tx, cursor_rx) =
                tokio::sync::watch::channel::<Option<pairwire_protocol::CursorUpdate>>(None);
            #[cfg(not(feature = "gui"))]
            let _ = (&frame_rx, &cursor_rx);

            let no_gui = args.no_gui;

            // Run the network session in a background task
            let session_handle = rt.spawn(session::run_join(args, frame_tx, cursor_tx));

            if no_gui {
                // Headless mode: just wait for the session to end
                rt.block_on(async {
                    match session_handle.await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => eprintln!("session error: {e}"),
                        Err(e) => eprintln!("task panic: {e}"),
                    }
                });
            } else {
                // GUI mode: run viewer on main thread (required by OS window systems)
                #[cfg(feature = "gui")]
                {
                    viewer::run_viewer(frame_rx, cursor_rx, "pairwire")?;
                }
                #[cfg(not(feature = "gui"))]
                {
                    eprintln!("GUI not available — rebuild with --features gui");
                    eprintln!("Running in headless mode (--no-gui)");
                    rt.block_on(async {
                        match session_handle.await {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => eprintln!("session error: {e}"),
                            Err(e) => eprintln!("task panic: {e}"),
                        }
                    });
                }
            }
        }
    }

    Ok(())
}
