//! pairwire — P2P screen sharing and pair programming.
//!
//! Usage:
//!   pairwire host [--fps N] [--width W] [--height H]
//!   pairwire join <invite-url> [--no-gui]

mod cli;
mod control_state;
mod guest_ctrl;
mod host_ctrl;
mod host_overlay_transparent;
// launcher is only used when the gui feature is compiled in (the non-gui
// run_launcher stub in main.rs does not need the module at all).
#[cfg(feature = "gui")]
mod launcher;
mod permissions;
mod reconnect;
mod session;
mod tray;
mod viewer;

use anyhow::Result;
use reconnect::{ConnectionState, ReconnectPolicy};
use tracing::info;

fn main() -> Result<()> {
    let cmd = cli::parse_args()?;

    // Handle non-runtime commands before initialising logging — running
    // `pairwire --version` or `pairwire --help` must not emit any log lines.
    match &cmd {
        cli::Command::Version => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        cli::Command::Help => {
            print!("{}", cli::HELP_TEXT);
            return Ok(());
        }
        _ => {}
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("pairwire=info".parse()?)
                .add_directive("iroh=warn".parse()?),
        )
        .init();

    info!(version = env!("CARGO_PKG_VERSION"), "pairwire starting");

    // Build the tokio runtime first so that `record()` can spawn tasks when
    // consent has already been given (e.g. on all launches after first-run).
    // AppLaunched is fired below, inside the runtime's context.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // Initialise telemetry. `_sentry_guard` must be kept alive for the process
    // lifetime — it flushes Sentry events on drop.
    let (telemetry_cfg, _sentry_guard) = pairwire_telemetry::init();

    // Fire AppLaunched inside the runtime so any tokio::spawn inside record()
    // has a live runtime to schedule onto.
    let _guard = rt.enter();
    pairwire_telemetry::record(pairwire_telemetry::TelemetryEvent::AppLaunched {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        os_family: std::env::consts::OS.to_owned(),
    });
    drop(_guard);

    match cmd {
        cli::Command::Host(args) => {
            run_host_command(args, &rt, telemetry_cfg)?;
        }
        cli::Command::Join(args) => {
            run_join_command(args, &rt)?;
        }
        // Handled above (pre-runtime early returns at lines 31–40); this arm
        // exists only for match exhaustiveness — do not remove the early returns
        // thinking this arm covers it, as the tokio runtime would then be built
        // unnecessarily before --version / --help output.
        cli::Command::Version | cli::Command::Help => {}
        cli::Command::Launcher => run_launcher(&rt, telemetry_cfg)?,
    }

    Ok(())
}

/// Show the native Host/Join chooser and dispatch to the appropriate handler.
///
/// When the `gui` feature is enabled the chooser uses a native `NSAlert` so it
/// does not consume winit's `EventLoop`; the host overlay / viewer `eframe` loop
/// starts cleanly after the choice is made.
#[cfg(feature = "gui")]
fn run_launcher(
    rt: &tokio::runtime::Runtime,
    telemetry_cfg: pairwire_telemetry::TelemetryConfig,
) -> Result<()> {
    match launcher::show_launcher()? {
        Some(launcher::LaunchChoice::Host(args)) => run_host_command(args, rt, telemetry_cfg),
        Some(launcher::LaunchChoice::Join(args)) => run_join_command(args, rt),
        None => {
            info!("launcher: cancelled");
            Ok(())
        }
    }
}

#[cfg(not(feature = "gui"))]
fn run_launcher(
    _rt: &tokio::runtime::Runtime,
    _telemetry_cfg: pairwire_telemetry::TelemetryConfig,
) -> Result<()> {
    anyhow::bail!(
        "no subcommand given — rebuild with --features gui for the launch chooser, \
         or run `pairwire host` / `pairwire join <invite>`"
    )
}

fn run_host_command(
    args: cli::HostArgs,
    rt: &tokio::runtime::Runtime,
    telemetry_cfg: pairwire_telemetry::TelemetryConfig,
) -> Result<()> {
    // ── Screen Recording permission gate (macOS + capture feature only) ───────
    // Check and, if necessary, request the macOS TCC Screen Recording permission
    // BEFORE launching the overlay or session. If not granted, the function
    // shows a native guidance dialog and returns false — we exit gracefully here
    // so the user never sees the blue-screen error that would occur later when
    // the capture thread fails to start inside run_host().
    //
    // This gate covers BOTH the `pairwire host` CLI path and the Launcher's
    // "Host" choice, since both paths call this function.
    #[cfg(feature = "capture")]
    {
        if !permissions::ensure_screen_recording_or_guide() {
            return Ok(());
        }
    }

    // Three valid configurations:
    //   1. gui (overlay on main thread, tray ignored if compiled in)
    //   2. tray only — no gui (tray on main thread)
    //   3. no overlay, no tray (headless: just run the session)
    //
    // On macOS both the tray (NSStatusBar) and winit's `EventLoop` insist on the
    // main thread. `eframe::run_native` seizes it permanently, so they cannot run
    // as two separate loops. In a `gui + tray` build we therefore create the tray
    // *inside* the overlay process and poll it from the eframe loop, so the host
    // gets the drawing overlay AND a menu-bar control icon at once.
    #[cfg(feature = "gui")]
    {
        if args.no_overlay {
            run_host_no_overlay(args, rt)
        } else {
            #[cfg(feature = "tray")]
            {
                run_host_with_overlay_and_tray(args, rt, telemetry_cfg)
            }
            #[cfg(not(feature = "tray"))]
            {
                run_host_with_overlay(args, rt, telemetry_cfg)
            }
        }
    }

    #[cfg(all(feature = "tray", not(feature = "gui")))]
    {
        let _ = telemetry_cfg; // no overlay in tray-only builds
        run_host_with_tray(args, rt)
    }

    #[cfg(not(any(feature = "gui", feature = "tray")))]
    {
        let _ = telemetry_cfg; // no overlay in headless builds
        run_host_no_overlay(args, rt)
    }
}

#[cfg(any(feature = "gui", not(feature = "tray")))]
fn run_host_no_overlay(args: cli::HostArgs, rt: &tokio::runtime::Runtime) -> Result<()> {
    // No tray/overlay = no runtime mute toggle. Initial state from `--mute`
    // sticks for the lifetime of the session.
    let (_mute_tx, mute_rx) = tokio::sync::watch::channel(args.mute);
    rt.block_on(session::run_host(args, None, None, mute_rx, None, None))
}

#[cfg(all(feature = "gui", not(feature = "tray")))]
fn run_host_with_overlay(
    args: cli::HostArgs,
    rt: &tokio::runtime::Runtime,
    telemetry_cfg: pairwire_telemetry::TelemetryConfig,
) -> Result<()> {
    // Spawn the session on the runtime; block on the main thread waiting for
    // the overlay channels, then launch the transparent overlay (which owns
    // the main thread for the rest of the session).
    let (overlay_tx, overlay_rx) = tokio::sync::oneshot::channel::<session::HostOverlayChannels>();
    let (host_shutdown_tx, host_shutdown_rx) = tokio::sync::watch::channel(false);
    // GUI overlay path has no in-session mute UI today (no tray, the overlay
    // is annotation-focused). `--mute` still applies the initial state.
    let (_mute_tx, mute_rx) = tokio::sync::watch::channel(args.mute);
    let session_handle = rt.spawn(session::run_host(
        args,
        None,
        None,
        mute_rx,
        Some(overlay_tx),
        Some(host_shutdown_rx),
    ));

    match rt.block_on(overlay_rx) {
        Ok(channels) => {
            let cw = channels.capture_width;
            let ch = channels.capture_height;
            if let Err(e) = host_overlay_transparent::run_transparent_host_overlay(
                channels,
                cw,
                ch,
                telemetry_cfg,
            ) {
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

/// Host with the transparent overlay AND a menu-bar tray in a single process.
///
/// The session is spawned with both the invite/control plumbing (for the tray)
/// and the overlay channels. The overlay owns the main thread and carries the
/// tray, polling it inside its eframe loop. Overlay channels are now sent before
/// the accept loop, so both the overlay and the tray appear the moment hosting
/// starts — the invite link is copyable before any guest connects.
#[cfg(all(feature = "gui", feature = "tray"))]
fn run_host_with_overlay_and_tray(
    args: cli::HostArgs,
    rt: &tokio::runtime::Runtime,
    telemetry_cfg: pairwire_telemetry::TelemetryConfig,
) -> Result<()> {
    let (overlay_tx, overlay_rx) = tokio::sync::oneshot::channel::<session::HostOverlayChannels>();
    let (invite_tx, invite_rx) = tokio::sync::oneshot::channel::<String>();
    let (host_shutdown_tx, host_shutdown_rx) = tokio::sync::watch::channel(false);
    // Overlay → session control actions (Grant / Revoke from the tray menu).
    let (control_tx, control_rx) = tokio::sync::mpsc::channel::<control_state::ControlAction>(8);

    let session_handle = rt.spawn(session::run_host(
        args,
        Some(invite_tx),
        Some(control_rx),
        Some(overlay_tx),
        Some(host_shutdown_rx),
    ));

    let channels = match rt.block_on(overlay_rx) {
        Ok(channels) => channels,
        Err(_) => {
            tracing::warn!("session ended before overlay channels were sent");
            let _ = host_shutdown_tx.send(true);
            match rt.block_on(session_handle) {
                Ok(Ok(())) | Err(_) => {}
                Ok(Err(e)) => eprintln!("session error: {e}"),
            }
            return Ok(());
        }
    };
    // Both `invite_tx` and `overlay_tx` fire before the accept loop, so this is
    // already buffered and resolves immediately.
    let invite_url = rt.block_on(invite_rx).ok();
    let cw = channels.capture_width;
    let ch = channels.capture_height;

    let tray_cfg = host_overlay_transparent::TrayConfig {
        invite_url,
        control_tx,
        shutdown_tx: host_shutdown_tx.clone(),
    };

    if let Err(e) = host_overlay_transparent::run_transparent_host_overlay_with_tray(
        channels,
        cw,
        ch,
        tray_cfg,
        telemetry_cfg,
    ) {
        tracing::warn!(error = %e, "transparent overlay (with tray) exited with error");
    }

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

    // Mute watch: seeded from `--mute`, toggled at runtime by the tray menu.
    let (mute_tx, mute_rx) = tokio::sync::watch::channel(args.mute);
    tray_handle.set_muted(args.mute);

    let session_handle = rt.spawn(session::run_host(
        args,
        Some(invite_tx),
        Some(tray_action_rx),
        mute_rx,
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
                TrayAction::ToggleMute => {
                    let new_state = !*mute_tx.borrow();
                    let _ = mute_tx.send(new_state);
                    tray_handle.set_muted(new_state);
                    info!(muted = new_state, "tray: mute toggled");
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
    // Seeded with `--mute` so the guest's mic starts muted when the flag is set.
    let (mute_tx, mute_rx_for_session) = tokio::sync::watch::channel(args.mute);
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
    #[cfg(feature = "gui")]
    let initial_muted = args.mute;

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
                initial_muted,
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
