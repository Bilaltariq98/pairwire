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

/// Build a [`JoinArgs`] from a raw invite string entered by the user.
///
/// Trims whitespace and delegates validation to [`crate::cli::validate_invite`].
/// Extracted as a pure function so it can be tested without AppKit.
pub fn make_join_args(invite: &str) -> anyhow::Result<JoinArgs> {
    let trimmed = invite.trim().to_string();
    crate::cli::validate_invite(&trimmed)?;
    Ok(JoinArgs {
        invite_url: trimmed,
        no_gui: false,
    })
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
    use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, ns_string};

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
    // Fix: give the field a real 320×24pt frame so it is visible inside the
    // NSAlert panel; a zero-sized view is invisible and receives no focus.
    // Give the field a real 320×24pt frame — a zero-sized view is invisible
    // and receives no keyboard focus inside NSAlert.
    // setFrame is a safe method on NSView (grandparent of NSTextField via
    // NSTextField → NSControl → NSView). The Deref chain surfaces it on `field`.
    let field = NSTextField::new(mtm);
    field.setFrame(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(320.0, 24.0),
    ));
    // Make it a standard editable text input with a bezel.
    // setBezeled/setEditable/setSelectable are safe fns on NSTextField in
    // objc2-app-kit 0.3 (the `#[unsafe(method(...))]` attr governs the ObjC
    // dispatch machinery, not the Rust-side safety of the generated wrapper).
    field.setBezeled(true);
    field.setEditable(true);
    field.setSelectable(true);
    field.setPlaceholderString(Some(ns_string!("pairwire://… (only needed to Join)")));

    // Pre-fill from clipboard when it looks like an invite URL, so most users
    // can just click Join without pasting manually. Any clipboard error is
    // silently ignored — the field just stays empty.
    if let Ok(mut cb) = arboard::Clipboard::new() {
        if let Ok(text) = cb.get_text() {
            if text.trim().starts_with("pairwire://") {
                // setStringValue is a safe fn on NSControl (superclass of NSTextField).
                field.setStringValue(&objc2_foundation::NSString::from_str(text.trim()));
                tracing::debug!("launcher: pre-filled invite field from clipboard");
            }
        }
    }

    // NSTextField inherits NSView via NSControl; the Deref chain (NSTextField →
    // NSControl → NSView) lets us borrow it as &NSView for setAccessoryView.
    alert.setAccessoryView(Some(&**field));

    // Focus the field so Cmd+V / typing work immediately without an extra click.
    // setInitialFirstResponder is gated on cfg(feature = "NSView") in NSWindow.rs
    // and takes Option<&NSView>; &**field derefs NSTextField → NSControl → NSView.
    // alert.window() returns Retained<NSWindow> valid for this stack frame;
    // the &NSView ref is valid as long as `field` is alive (it outlives runModal).
    alert.window().setInitialFirstResponder(Some(&**field));

    let response = alert.runModal();

    if response == NSAlertFirstButtonReturn {
        // "Host" — launch with default host settings.
        tracing::debug!("launcher: Host chosen");
        Ok(Some(LaunchChoice::Host(HostArgs::default())))
    } else if response == NSAlertSecondButtonReturn {
        // "Join…" — validate the invite URL from the accessory field.
        let invite = field.stringValue().to_string();
        match make_join_args(&invite) {
            Ok(args) => {
                tracing::debug!("launcher: Join chosen");
                Ok(Some(LaunchChoice::Join(args)))
            }
            Err(e) => {
                // Show a simple error alert. The process will exit after the
                // dialog — the informative text tells the user to relaunch.
                let err_alert = NSAlert::new(mtm);
                err_alert.setMessageText(ns_string!("Invalid invite link"));
                err_alert.setInformativeText(ns_string!(
                    "Paste a link that starts with pairwire://\nPlease relaunch Pairwire to try again."
                ));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_join_args_accepts_valid_invite() {
        let args = make_join_args("pairwire://abc?token=0000").unwrap();
        assert_eq!(args.invite_url, "pairwire://abc?token=0000");
        assert!(!args.no_gui, "no_gui must default to false");
    }

    #[test]
    fn make_join_args_trims_whitespace() {
        let args = make_join_args("  pairwire://abc  ").unwrap();
        assert_eq!(args.invite_url, "pairwire://abc");
    }

    #[test]
    fn make_join_args_rejects_empty_string() {
        assert!(make_join_args("").is_err());
        assert!(make_join_args("   ").is_err());
    }

    #[test]
    fn make_join_args_rejects_non_pairwire_scheme() {
        assert!(make_join_args("https://example.com/invite").is_err());
        assert!(make_join_args("pairwire-copy://abc").is_err());
    }
}
