//! pairwire — P2P screen sharing and pair programming.
//!
//! Usage:
//!   pairwire host [--fps N] [--width W] [--height H]
//!   pairwire join <invite-url> [--no-gui]

mod cli;
mod reconnect;
mod session;
mod tray;
mod viewer;

use anyhow::Result;
use reconnect::{ConnectionState, ReconnectPolicy};
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
            run_host_command(args, &rt)?;
        }
        cli::Command::Join(args) => {
            run_join_command(args, &rt)?;
        }
    }

    Ok(())
}

fn run_host_command(args: cli::HostArgs, rt: &tokio::runtime::Runtime) -> Result<()> {
    #[cfg(not(feature = "tray"))]
    {
        // Without the tray feature, run the session directly on the async runtime.
        rt.block_on(session::run_host(args, None))
    }

    #[cfg(feature = "tray")]
    {
        // With the tray feature: spawn the session as a background task so the
        // main thread can drive the tray event loop (macOS requirement).
        use tray::TrayAction;

        let (invite_tx, invite_rx) = tokio::sync::oneshot::channel::<String>();
        let mut tray_handle = tray::TrayHandle::new(None)?;

        let session_handle = rt.spawn(session::run_host(args, Some(invite_tx)));

        // Block until the invite URL arrives, then update the tray item.
        if let Ok(url) = rt.block_on(invite_rx) {
            tray_handle.set_invite_url(url);
        }

        // Poll tray events on the main thread until the session ends.
        loop {
            if let Some(action) = tray_handle.poll() {
                match action {
                    TrayAction::CopyInvite(url) => {
                        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(url.clone()))
                        {
                            Ok(()) => info!(url, "invite link copied to clipboard"),
                            Err(e) => tracing::warn!(error = %e, "clipboard write failed"),
                        }
                    }
                    TrayAction::Disconnect => {
                        info!("tray: disconnect requested");
                        session_handle.abort();
                        break;
                    }
                    TrayAction::Quit => {
                        info!("tray: quit requested");
                        session_handle.abort();
                        std::process::exit(0);
                    }
                }
            }

            if session_handle.is_finished() {
                break;
            }

            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        match rt.block_on(session_handle) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("session error: {e}"),
            Err(e) if e.is_cancelled() => {}
            Err(e) => eprintln!("task panic: {e}"),
        }

        Ok(())
    }
}

#[allow(clippy::unnecessary_wraps)] // Result is needed when `gui` feature propagates viewer errors
fn run_join_command(args: cli::JoinArgs, rt: &tokio::runtime::Runtime) -> Result<()> {
    let (frame_tx, frame_rx) =
        tokio::sync::watch::channel::<Option<pairwire_capture::DecodedFrame>>(None);
    let (cursor_tx, cursor_rx) =
        tokio::sync::watch::channel::<Option<pairwire_protocol::CursorUpdate>>(None);
    let (conn_tx, conn_rx) = tokio::sync::watch::channel(ConnectionState::Connected);

    #[cfg(not(feature = "gui"))]
    let _ = (&frame_rx, &cursor_rx, &conn_rx);

    let no_gui = args.no_gui;

    // Run the network session in a background task, wrapped in reconnect logic.
    let session_handle = rt.spawn(async move {
        let policy = ReconnectPolicy::default();
        reconnect::with_reconnect(policy, conn_tx, move || {
            let frame_tx = frame_tx.clone();
            let cursor_tx = cursor_tx.clone();
            let args = args.clone();
            async move { session::run_join(args, frame_tx, cursor_tx).await }
        })
        .await
    });

    if no_gui {
        rt.block_on(async {
            match session_handle.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => eprintln!("session error: {e}"),
                Err(e) => eprintln!("task panic: {e}"),
            }
        });
    } else {
        #[cfg(feature = "gui")]
        {
            viewer::run_viewer(frame_rx, cursor_rx, conn_rx, "pairwire")?;
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

    Ok(())
}
