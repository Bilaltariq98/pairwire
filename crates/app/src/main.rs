//! pairwire — P2P screen sharing and pair programming.
//!
//! Usage:
//!   pairwire host [--fps N] [--width W] [--height H]
//!   pairwire join <invite-url> [--no-gui]

mod cli;
mod config_cmd;
mod control_state;
mod guest_ctrl;
mod host_ctrl;
mod host_overlay_transparent;
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

    // `config` subcommands are synchronous file ops — no tokio runtime
    // needed. Handle them before constructing the runtime.
    if let cli::Command::Config(args) = cmd {
        return config_cmd::run(args);
    }

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
        cli::Command::Config(_) => unreachable!("handled above"),
    }

    Ok(())
}

fn run_host_command(args: cli::HostArgs, rt: &tokio::runtime::Runtime) -> Result<()> {
    // Three valid configurations:
    //   1. gui (overlay on main thread, tray ignored if compiled in)
    //   2. tray only — no gui (tray on main thread)
    //   3. no overlay, no tray (headless: just run the session)
    //
    // On macOS both the tray (NSStatusBar) and winit's `EventLoop` insist on
    // being created on the main thread. They cannot share it. When `gui` is
    // compiled in we always give the overlay main-thread ownership; the tray
    // is silently ignored. To run with tray+menubar control, build without `gui`.
    #[cfg(feature = "gui")]
    {
        if args.no_overlay {
            run_host_no_overlay(args, rt)
        } else {
            run_host_with_overlay(args, rt)
        }
    }

    #[cfg(all(feature = "tray", not(feature = "gui")))]
    {
        run_host_with_tray(args, rt)
    }

    #[cfg(not(any(feature = "gui", feature = "tray")))]
    {
        run_host_no_overlay(args, rt)
    }
}

#[cfg(any(feature = "gui", not(feature = "tray")))]
fn run_host_no_overlay(args: cli::HostArgs, rt: &tokio::runtime::Runtime) -> Result<()> {
    rt.block_on(session::run_host(args, None, None, None, None))
}

#[cfg(feature = "gui")]
fn run_host_with_overlay(args: cli::HostArgs, rt: &tokio::runtime::Runtime) -> Result<()> {
    // Spawn the session on the runtime; block on the main thread waiting for
    // the overlay channels, then launch the transparent overlay (which owns
    // the main thread for the rest of the session).
    let (overlay_tx, overlay_rx) = tokio::sync::oneshot::channel::<session::HostOverlayChannels>();
    let (host_shutdown_tx, host_shutdown_rx) = tokio::sync::watch::channel(false);
    let session_handle = rt.spawn(session::run_host(
        args,
        None,
        None,
        Some(overlay_tx),
        Some(host_shutdown_rx),
    ));

    match rt.block_on(overlay_rx) {
        Ok(channels) => {
            let cw = channels.capture_width;
            let ch = channels.capture_height;
            if let Err(e) = host_overlay_transparent::run_transparent_host_overlay(channels, cw, ch)
            {
                tracing::warn!(error = %e, "transparent overlay exited with error");
            }
        }
        Err(_) => {
            tracing::warn!("session ended before overlay channels were sent");
        }
    }

    // Signal `run_host` to shut down gracefully so it can call
    // `transport.close().await` before the tokio runtime is dropped.
    let _ = host_shutdown_tx.send(true);
    match rt.block_on(session_handle) {
        Ok(Ok(())) | Err(_) => {}
        Ok(Err(e)) => eprintln!("session error: {e}"),
    }
    Ok(())
}

#[cfg(all(feature = "tray", not(feature = "gui")))]
fn run_host_with_tray(args: cli::HostArgs, rt: &tokio::runtime::Runtime) -> Result<()> {
    use tray::TrayAction;

    let (invite_tx, invite_rx) = tokio::sync::oneshot::channel::<String>();
    let mut tray_handle = tray::TrayHandle::new(None)?;

    let (tray_action_tx, tray_action_rx) =
        tokio::sync::mpsc::channel::<control_state::ControlAction>(8);

    let session_handle = rt.spawn(session::run_host(
        args,
        Some(invite_tx),
        Some(tray_action_rx),
        None,
        None,
    ));

    if let Ok(url) = rt.block_on(invite_rx) {
        tray_handle.set_invite_url(url);
    }

    loop {
        if let Some(action) = tray_handle.poll() {
            match action {
                TrayAction::CopyInvite(url) => {
                    match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(url.clone())) {
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
                TrayAction::GrantControl => {
                    let _ = tray_action_tx.try_send(control_state::ControlAction::Grant);
                    tray_handle.set_control_granted(true);
                    info!("tray: grant control forwarded to session");
                }
                TrayAction::RevokeControl => {
                    let _ = tray_action_tx.try_send(control_state::ControlAction::Revoke);
                    tray_handle.set_control_granted(false);
                    info!("tray: revoke control forwarded to session");
                }
            }
        }

        if session_handle.is_finished() {
            break;
        }

        #[cfg(target_os = "macos")]
        {
            use objc2_app_kit::{NSApplication, NSEventMask};
            use objc2_foundation::{MainThreadMarker, NSDate};

            let mtm = MainThreadMarker::new().expect("must be main thread");
            let app = NSApplication::sharedApplication(mtm);
            loop {
                let event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    Some(&NSDate::dateWithTimeIntervalSinceNow(0.05)),
                    objc2_foundation::ns_string!("kCFRunLoopDefaultMode"),
                    true,
                );
                match event {
                    Some(event) => app.sendEvent(&event),
                    None => break,
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
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

#[allow(clippy::unnecessary_wraps)] // Result is needed when `gui` feature propagates viewer errors
fn run_join_command(args: cli::JoinArgs, rt: &tokio::runtime::Runtime) -> Result<()> {
    let (frame_tx, frame_rx) =
        tokio::sync::watch::channel::<Option<pairwire_capture::DecodedFrame>>(None);
    let (cursor_tx, cursor_rx) =
        tokio::sync::watch::channel::<Option<pairwire_protocol::CursorUpdate>>(None);
    let (conn_tx, conn_rx) = tokio::sync::watch::channel(ConnectionState::Connected);
    let conn_tx = std::sync::Arc::new(conn_tx);
    let conn_tx_for_session = std::sync::Arc::clone(&conn_tx);
    // Drawing strokes watch: session publishes accumulated strokes, viewer renders them.
    let (drawing_strokes_tx, drawing_strokes_rx) =
        tokio::sync::watch::channel::<Vec<pairwire_transport::TimedStroke>>(Vec::new());
    // Live DrawPoint preview: receiver publishes the latest in-flight point per peer.
    let (live_point_tx, live_point_rx) =
        tokio::sync::watch::channel::<Option<pairwire_protocol::drawing::DrawPoint>>(None);
    // Input channel: viewer sends InputMessage events, session forwards to host.
    // Uses broadcast so each reconnect attempt can subscribe independently.
    let (input_tx, _) =
        tokio::sync::broadcast::channel::<pairwire_protocol::input::InputMessage>(32);
    let input_tx_for_session = input_tx.clone();
    // Drawing event channel: viewer sends DrawingEvent (strokes, points, clears) to session.
    // Uses broadcast so each reconnect attempt can subscribe independently.
    let (drawing_event_tx, _) =
        tokio::sync::broadcast::channel::<pairwire_transport::DrawingEvent>(32);
    let drawing_event_tx_for_session = drawing_event_tx.clone();
    // Mute channel: viewer sends mute state (true=muted), session applies to capture thread.
    let (mute_tx, mute_rx_for_session) = tokio::sync::watch::channel(false);
    // Quality tier: session publishes current connection quality, viewer shows HUD dot.
    let (quality_tx_for_session, quality_rx) =
        tokio::sync::watch::channel(pairwire_transport::QualityTier::Full);
    // Control state: session publishes the current RuntimeControlState, viewer shows HUD label.
    let (control_state_tx_for_session, control_state_rx) =
        tokio::sync::watch::channel(crate::control_state::RuntimeControlState::HostActive);
    // Control request: viewer sends ControlRequest messages, session forwards to host.
    // Uses broadcast so each reconnect attempt can subscribe independently.
    let (control_request_tx, _) =
        tokio::sync::broadcast::channel::<pairwire_protocol::ControlMessage>(8);
    let control_request_tx_for_session = control_request_tx.clone();
    // Graceful-shutdown channel: the GUI sets this to `true` when the window
    // closes so `run_join` can run its normal teardown (including
    // `transport.close().await`) instead of being aborted mid-flight.
    let (shutdown_tx, shutdown_rx_for_session) = tokio::sync::watch::channel(false);

    #[cfg(not(feature = "gui"))]
    let _ = (
        &frame_rx,
        &cursor_rx,
        &conn_rx,
        &input_tx,
        &drawing_event_tx,
        &drawing_strokes_rx,
        &live_point_rx,
        &mute_tx,
        &mute_rx_for_session,
        &quality_tx_for_session,
        &quality_rx,
        &control_state_rx,
        &control_request_tx,
        &shutdown_tx,
    );

    let no_gui = args.no_gui;

    // Run the network session in a background task, wrapped in reconnect logic.
    let session_handle = rt.spawn(async move {
        let policy = ReconnectPolicy::default();
        reconnect::with_reconnect(policy, conn_tx, move || {
            let conn_tx_for_session = std::sync::Arc::clone(&conn_tx_for_session);
            let frame_tx = frame_tx.clone();
            let cursor_tx = cursor_tx.clone();
            let drawing_strokes_tx = drawing_strokes_tx.clone();
            let live_point_tx = live_point_tx.clone();
            let args = args.clone();
            let mute_rx = mute_rx_for_session.clone();
            let quality_tx = quality_tx_for_session.clone();
            let control_state_tx = control_state_tx_for_session.clone();
            let shutdown_rx = shutdown_rx_for_session.clone();
            // Each reconnect gets a fresh broadcast subscriber → mpsc bridge.
            let mut input_broadcast_rx = input_tx_for_session.subscribe();
            let mut drawing_broadcast_rx = drawing_event_tx_for_session.subscribe();
            let mut control_request_broadcast_rx = control_request_tx_for_session.subscribe();
            async move {
                let (input_bridge_tx, input_bridge_rx) =
                    tokio::sync::mpsc::channel::<pairwire_protocol::input::InputMessage>(32);
                tokio::spawn(async move {
                    while let Ok(msg) = input_broadcast_rx.recv().await {
                        if input_bridge_tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                });
                // Bridge drawing events broadcast → mpsc for this session attempt.
                // DrawingSender inside session consumes the mpsc receiver.
                let (drawing_bridge_tx, drawing_bridge_rx) =
                    tokio::sync::mpsc::channel::<pairwire_transport::DrawingEvent>(32);
                tokio::spawn(async move {
                    while let Ok(event) = drawing_broadcast_rx.recv().await {
                        if drawing_bridge_tx.send(event).await.is_err() {
                            break;
                        }
                    }
                });
                // Bridge control request broadcast → mpsc for this session attempt.
                let (control_request_bridge_tx, control_request_bridge_rx) =
                    tokio::sync::mpsc::channel::<pairwire_protocol::ControlMessage>(8);
                tokio::spawn(async move {
                    while let Ok(msg) = control_request_broadcast_rx.recv().await {
                        if control_request_bridge_tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                });
                session::run_join(
                    args,
                    frame_tx,
                    cursor_tx,
                    drawing_strokes_tx,
                    live_point_tx,
                    drawing_bridge_rx,
                    mute_rx,
                    quality_tx,
                    input_bridge_rx,
                    control_state_tx,
                    control_request_bridge_rx,
                    shutdown_rx,
                    conn_tx_for_session,
                )
                .await
            }
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
            let viewer_result = viewer::run_viewer(
                frame_rx,
                cursor_rx,
                conn_rx,
                input_tx,
                drawing_strokes_rx,
                live_point_rx,
                drawing_event_tx,
                mute_tx,
                quality_rx,
                control_state_rx,
                control_request_tx,
                "pairwire",
            );

            // The GUI owns the foreground lifetime. When the window closes,
            // signal `run_join` to shut down gracefully so it can call
            // `transport.close().await` before the tokio runtime is dropped.
            // This ensures the peer receives a proper QUIC CONNECTION_CLOSE
            // frame instead of relying on the idle-timeout to detect the drop.
            let _ = shutdown_tx.send(true);
            rt.block_on(async {
                let mut session_handle = session_handle;
                match tokio::time::timeout(std::time::Duration::from_secs(5), &mut session_handle)
                    .await
                {
                    Ok(Ok(Ok(()))) => {}
                    Ok(Ok(Err(e))) => eprintln!("session error: {e}"),
                    Ok(Err(e)) if e.is_cancelled() => {}
                    Ok(Err(e)) => eprintln!("task panic: {e}"),
                    Err(_) => {
                        tracing::warn!(
                            "session did not finish within 5s after viewer close; aborting"
                        );
                        session_handle.abort();
                        let _ = session_handle.await;
                    }
                }
            });

            viewer_result?;
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
