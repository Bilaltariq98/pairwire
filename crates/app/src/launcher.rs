//! Native launch chooser shown when pairwire is started with no subcommand
//! (e.g. double-clicked from /Applications). MUST be native (NSAlert), not
//! eframe — winit allows only one EventLoop per process and the host/viewer
//! consume it after the choice is made.
//!
//! On non-macOS platforms the function returns an error directing the user to
//! use a subcommand; those platforms are not a v1 target.

use crate::cli::{HostArgs, JoinArgs};

/// What the user chose in the launch dialog.
#[derive(Debug)]
pub enum LaunchChoice {
    Host(HostArgs),
    Join(JoinArgs),
}

/// Show the native Host/Join chooser and return the user's choice.
///
/// Returns `Ok(None)` when the user cancels or dismisses the dialog.
/// Must be called from the main thread (AppKit requirement).
#[cfg(target_os = "macos")]
pub fn show_launcher() -> anyhow::Result<Option<LaunchChoice>> {
    use objc2_app_kit::{
        NSAlert, NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSApplication,
        NSApplicationActivationPolicy, NSTextField,
    };
    use objc2_foundation::{MainThreadMarker, ns_string};

    let mtm = MainThreadMarker::new()
        .ok_or_else(|| anyhow::anyhow!("launcher must run on the main thread"))?;

    // Bring the process to the foreground so the NSAlert becomes frontmost.
    // For an accessory (LSUIElement=true) bundle the dialog would otherwise be
    // hidden behind the active app. We set Regular here; the tray/host overlay
    // setup will restore Accessory after the choice is made.
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    // Build the main chooser alert.
    let alert = NSAlert::new(mtm);
    alert.setMessageText(ns_string!("Pairwire"));
    alert.setInformativeText(ns_string!(
        "Host a session to share your screen, or paste an invite link to join."
    ));
    // Buttons are assigned return codes in add order:
    //   NSAlertFirstButtonReturn  (1000) → "Host"
    //   NSAlertSecondButtonReturn (1001) → "Join…"
    //   NSAlertThirdButtonReturn  (1002) → "Cancel"
    alert.addButtonWithTitle(ns_string!("Host"));
    alert.addButtonWithTitle(ns_string!("Join…"));
    alert.addButtonWithTitle(ns_string!("Cancel"));

    // Accessory text field for the invite link (only consumed for Join).
    let field = NSTextField::new(mtm);
    field.setPlaceholderString(Some(ns_string!("pairwire://… (only needed to Join)")));
    // NSTextField inherits NSView via NSControl; the Deref chain (NSTextField →
    // NSControl → NSView) lets us borrow it as &NSView for setAccessoryView.
    alert.setAccessoryView(Some(&**field));

    let response = alert.runModal();

    if response == NSAlertFirstButtonReturn {
        // "Host" — launch with default host settings.
        tracing::debug!("launcher: Host chosen");
        Ok(Some(LaunchChoice::Host(HostArgs::default())))
    } else if response == NSAlertSecondButtonReturn {
        // "Join…" — validate the invite URL from the accessory field.
        let invite = field.stringValue().to_string();
        match crate::cli::validate_invite(&invite) {
            Ok(()) => {
                tracing::debug!("launcher: Join chosen");
                Ok(Some(LaunchChoice::Join(JoinArgs {
                    invite_url: invite.trim().to_string(),
                    no_gui: false,
                })))
            }
            Err(e) => {
                // Show a simple error alert and return None (user can relaunch).
                let err_alert = NSAlert::new(mtm);
                err_alert.setMessageText(ns_string!("Invalid invite link"));
                err_alert
                    .setInformativeText(ns_string!("Paste a link that starts with pairwire://"));
                err_alert.addButtonWithTitle(ns_string!("OK"));
                err_alert.runModal();
                tracing::warn!(error = %e, "launcher: invalid invite link entered");
                Ok(None)
            }
        }
    } else {
        // "Cancel" or any other response.
        tracing::debug!("launcher: cancelled");
        Ok(None)
    }
}

/// Non-macOS stub — v1 targets macOS only.
#[cfg(not(target_os = "macos"))]
pub fn show_launcher() -> anyhow::Result<Option<LaunchChoice>> {
    anyhow::bail!("no subcommand given — run `pairwire host` or `pairwire join <invite>`")
}
