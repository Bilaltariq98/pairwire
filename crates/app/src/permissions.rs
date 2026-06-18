//! macOS Screen Recording permission gate.
//!
//! Call [`ensure_screen_recording_or_guide`] at the top of the host path to
//! check (and optionally request) Screen Recording permission before launching
//! the overlay or session. If permission is not granted, a native `NSAlert`
//! guides the user to System Settings → Privacy & Security → Screen Recording
//! and returns `false` so the caller can exit gracefully without showing the
//! blue-screen error.
//!
//! # macOS restart requirement
//!
//! macOS (ScreenCaptureKit) does **not** pick up a newly-granted Screen
//! Recording permission until the app relaunches. The dialog therefore tells
//! the user to enable the permission and reopen Pairwire rather than retrying
//! in the same process.

/// The System Settings deep-link URL for the Screen Recording privacy pane.
/// Opening this URL with `open(1)` takes the user directly to the right panel.
#[cfg(all(feature = "capture", target_os = "macos"))]
pub(crate) const SCREEN_RECORDING_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

/// Returns `true` if the host session may proceed.
///
/// On macOS with the `capture` feature enabled, this function:
///
/// 1. Checks whether Screen Recording permission is already granted via
///    `scap::has_permission()`.  If yes, returns `true` immediately.
/// 2. Calls `scap::request_permission()` to trigger the TCC dialog (shows only
///    on the first run; returns the cached decision on subsequent runs).
/// 3. If permission is still not granted after the request, shows a native
///    `NSAlert` that explains the situation and offers two buttons:
///    - **"Open Settings"** — opens the Screen Recording pane in System
///      Settings so the user can enable the permission.
///    - **"Quit"** — dismisses without opening Settings.
///
///    Then returns `false` so the caller exits without launching the overlay.
///
/// When `capture` is not a compile-time feature (e.g. headless CI builds),
/// the stub below always returns `true` — no permission is needed.
///
/// # Main-thread requirement
///
/// On macOS the `NSAlert` must be shown from the main thread.  Call this
/// function from `run_host_command`, which runs on the main thread before any
/// overlay or async work begins.
#[cfg(feature = "capture")]
pub fn ensure_screen_recording_or_guide() -> bool {
    use pairwire_capture::{
        request_screen_recording_permission, screen_recording_permission_granted,
    };

    // Fast path: already authorised.
    if screen_recording_permission_granted() {
        return true;
    }

    // First run: trigger the TCC prompt (CGRequestScreenCaptureAccess).
    // On subsequent runs this returns the cached TCC decision without prompting.
    tracing::info!("requesting Screen Recording permission from macOS TCC");
    if request_screen_recording_permission() {
        // Permission was just granted. ScreenCaptureKit still won't pick it up
        // until the process restarts, so we fall through to the guidance dialog.
        tracing::info!("Screen Recording permission granted; showing restart-required notice");
        show_restart_required_alert();
        return false;
    }

    // Permission was denied (or the TCC dialog was dismissed without granting).
    tracing::warn!("Screen Recording permission not granted; showing guidance dialog");
    show_permission_guidance_alert();
    false
}

/// Stub for non-capture builds — no Screen Recording permission needed.
#[cfg(not(feature = "capture"))]
#[allow(dead_code)]
pub(crate) fn ensure_screen_recording_or_guide() -> bool {
    true
}

// ── Internal macOS alert helpers ─────────────────────────────────────────────
// These are only compiled when `capture` is enabled on macOS. They follow
// the same NSAlert pattern used in launcher.rs.

/// Shows a native NSAlert telling the user that Screen Recording permission
/// was not granted, with an "Open Settings" button that opens the TCC pane
/// and a "Quit" button that dismisses.
#[cfg(all(feature = "capture", target_os = "macos"))]
fn show_permission_guidance_alert() {
    use objc2_app_kit::{
        NSAlert, NSAlertFirstButtonReturn, NSApplication, NSApplicationActivationPolicy,
    };
    use objc2_foundation::{MainThreadMarker, ns_string};

    let mtm = match MainThreadMarker::new() {
        Some(m) => m,
        None => {
            tracing::error!("ensure_screen_recording_or_guide must be called from the main thread");
            return;
        }
    };

    // Bring the process to the foreground so the alert is frontmost.
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let alert = NSAlert::new(mtm);
    alert.setMessageText(ns_string!("Screen Recording Permission Required"));
    alert.setInformativeText(ns_string!(
        "Pairwire needs Screen Recording permission to share your screen.\n\n\
        Please enable Pairwire under:\n\
        System Settings → Privacy & Security → Screen Recording\n\n\
        Then reopen Pairwire to start hosting."
    ));
    // First button  → NSAlertFirstButtonReturn (1000)  → "Open Settings"
    // Second button → NSAlertSecondButtonReturn (1001) → "Quit"
    alert.addButtonWithTitle(ns_string!("Open Settings"));
    alert.addButtonWithTitle(ns_string!("Quit"));

    let response = alert.runModal();
    if response == NSAlertFirstButtonReturn {
        open_screen_recording_settings();
    }
}

/// Shows a native NSAlert informing the user that permission was just granted
/// but a restart is required for ScreenCaptureKit to pick it up.
#[cfg(all(feature = "capture", target_os = "macos"))]
fn show_restart_required_alert() {
    use objc2_app_kit::{NSAlert, NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::{MainThreadMarker, ns_string};

    let mtm = match MainThreadMarker::new() {
        Some(m) => m,
        None => {
            tracing::error!("ensure_screen_recording_or_guide must be called from the main thread");
            return;
        }
    };

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let alert = NSAlert::new(mtm);
    alert.setMessageText(ns_string!("Reopen Pairwire to Start Hosting"));
    alert.setInformativeText(ns_string!(
        "Screen Recording permission was granted. macOS requires Pairwire \
        to be relaunched before the new permission takes effect.\n\n\
        Please reopen Pairwire to start hosting."
    ));
    alert.addButtonWithTitle(ns_string!("OK"));

    // Ignore the return value — there is only one button.
    let _ = alert.runModal();
}

/// Opens the Screen Recording privacy pane in System Settings.
///
/// Uses `open(1)` with the deep-link URL. Errors are logged but not propagated
/// — the alert already told the user where to look.
#[cfg(all(feature = "capture", target_os = "macos"))]
fn open_screen_recording_settings() {
    match std::process::Command::new("open")
        .arg(SCREEN_RECORDING_SETTINGS_URL)
        .status()
    {
        Ok(status) => {
            tracing::info!(
                exit_code = status.code(),
                url = SCREEN_RECORDING_SETTINGS_URL,
                "opened Screen Recording settings pane"
            );
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                url = SCREEN_RECORDING_SETTINGS_URL,
                "failed to open Screen Recording settings pane"
            );
        }
    }
}

// ── Non-macOS stubs ───────────────────────────────────────────────────────────

#[cfg(all(feature = "capture", not(target_os = "macos")))]
fn show_permission_guidance_alert() {
    tracing::warn!(
        "screen capture permission not granted; \
        on non-macOS platforms this is unexpected"
    );
}

#[cfg(all(feature = "capture", not(target_os = "macos")))]
fn show_restart_required_alert() {
    tracing::info!("Screen Recording permission granted; please restart the application");
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    /// Verify that the Settings URL constant has the expected format.
    /// This is pure logic — no UI is shown.
    #[cfg(all(feature = "capture", target_os = "macos"))]
    #[test]
    fn screen_recording_settings_url_is_well_formed() {
        use super::SCREEN_RECORDING_SETTINGS_URL;
        assert!(
            SCREEN_RECORDING_SETTINGS_URL.starts_with("x-apple.systempreferences:"),
            "URL must use the x-apple.systempreferences scheme"
        );
        assert!(
            SCREEN_RECORDING_SETTINGS_URL.contains("Privacy_ScreenCapture"),
            "URL must target the Screen Capture privacy pane"
        );
    }
}
