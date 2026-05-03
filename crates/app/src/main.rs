//! pairwire — P2P screen sharing and pair programming.
//!
//! Usage:
//!   pairwire host [--fps N] [--width W] [--height H]
//!   pairwire join <invite-url> [--no-gui]

mod cli;
mod control_state;
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
        rt.block_on(session::run_host(args, None, None))
    }

    #[cfg(feature = "tray")]
    {
        // With the tray feature: spawn the session as a background task so the
        // main thread can drive the tray event loop (macOS requirement).
        use tray::TrayAction;

        let (invite_tx, invite_rx) = tokio::sync::oneshot::channel::<String>();
        let mut tray_handle = tray::TrayHandle::new(None)?;

        // Bridge tray GrantControl / RevokeControl events into the session's
        // control-state machine. The mpsc channel is created here (on the main
        // thread) and the receiver is passed into run_host.
        let (tray_action_tx, tray_action_rx) =
            tokio::sync::mpsc::channel::<control_state::ControlAction>(8);

        let session_handle = rt.spawn(session::run_host(
            args,
            Some(invite_tx),
            Some(tray_action_rx),
        ));

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

            // On macOS, the tray menu requires the NSApplication event
            // dispatch loop — CFRunLoopRunInMode only pumps CF sources, NOT
            // AppKit events (menu clicks, popups). We use the standard Cocoa
            // event pump: nextEventMatchingMask + sendEvent.
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
}

#[allow(clippy::unnecessary_wraps)] // Result is needed when `gui` feature propagates viewer errors
fn run_join_command(args: cli::JoinArgs, rt: &tokio::runtime::Runtime) -> Result<()> {
    let (frame_tx, frame_rx) =
        tokio::sync::watch::channel::<Option<pairwire_capture::DecodedFrame>>(None);
    let (cursor_tx, cursor_rx) =
        tokio::sync::watch::channel::<Option<pairwire_protocol::CursorUpdate>>(None);
    let (conn_tx, conn_rx) = tokio::sync::watch::channel(ConnectionState::Connected);
    // Drawing strokes watch: session publishes accumulated strokes, viewer renders them.
    let (drawing_strokes_tx, drawing_strokes_rx) =
        tokio::sync::watch::channel::<Vec<pairwire_protocol::drawing::DrawStroke>>(Vec::new());
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

    #[cfg(not(feature = "gui"))]
    let _ = (
        &frame_rx,
        &cursor_rx,
        &conn_rx,
        &input_tx,
        &drawing_event_tx,
        &drawing_strokes_rx,
        &mute_tx,
        &mute_rx_for_session,
        &quality_tx_for_session,
        &quality_rx,
        &control_state_rx,
        &control_request_tx,
    );

    let no_gui = args.no_gui;

    // Run the network session in a background task, wrapped in reconnect logic.
    let session_handle = rt.spawn(async move {
        let policy = ReconnectPolicy::default();
        reconnect::with_reconnect(policy, conn_tx, move || {
            let frame_tx = frame_tx.clone();
            let cursor_tx = cursor_tx.clone();
            let drawing_strokes_tx = drawing_strokes_tx.clone();
            let args = args.clone();
            let mute_rx = mute_rx_for_session.clone();
            let quality_tx = quality_tx_for_session.clone();
            let control_state_tx = control_state_tx_for_session.clone();
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
                    drawing_bridge_rx,
                    mute_rx,
                    quality_tx,
                    input_bridge_rx,
                    control_state_tx,
                    control_request_bridge_rx,
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
                drawing_event_tx,
                mute_tx,
                quality_rx,
                control_state_rx,
                control_request_tx,
                "pairwire",
            );

            // The GUI owns the foreground lifetime. When the window closes, stop
            // the background network task before dropping the Tokio runtime;
            // otherwise iroh's Endpoint is dropped without `Endpoint::close()`.
            session_handle.abort();
            rt.block_on(async {
                match session_handle.await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => eprintln!("session error: {e}"),
                    Err(e) if e.is_cancelled() => {}
                    Err(e) => eprintln!("task panic: {e}"),
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
