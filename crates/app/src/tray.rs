//! macOS menu bar (system tray) integration.
//!
//! Creates a menu bar icon with three items:
//! - "Copy Invite Link" — copies the current session invite URL to the clipboard
//! - "Disconnect"       — signals the active session to shut down
//! - "Quit"             — exits the process
//!
//! Only compiled when the `tray` feature is enabled. The tray icon must be
//! created on the main thread (macOS requirement). Event polling is done via
//! `MenuEvent::receiver().try_recv()` inside the eframe/egui update loop or
//! a dedicated `poll_tray_events` call from the main thread.
//!
//! # Usage
//!
//! ```no_run
//! # #[cfg(feature = "tray")]
//! # {
//! use pairwire::tray::{TrayHandle, TrayAction};
//!
//! let handle = TrayHandle::new(Some("pairwire://invite/...".to_string())).unwrap();
//!
//! // In your event loop:
//! if let Some(action) = handle.poll() {
//!     match action {
//!         TrayAction::CopyInvite(url) => { /* write to clipboard */ }
//!         TrayAction::Disconnect      => { /* signal session shutdown */ }
//!         TrayAction::Quit            => std::process::exit(0),
//!     }
//! }
//! # }
//! ```

#[cfg(feature = "tray")]
mod inner {
    use tracing::{debug, warn};
    use tray_icon::{
        TrayIconBuilder,
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };

    /// Actions emitted by the tray menu.
    #[derive(Debug, Clone)]
    pub enum TrayAction {
        /// User clicked "Copy Invite Link". Contains the invite URL.
        CopyInvite(String),
        /// User clicked "Disconnect".
        Disconnect,
        /// User clicked "Quit".
        Quit,
        /// User clicked "Allow Remote Control".
        GrantControl,
        /// User clicked "Revoke Control".
        RevokeControl,
    }

    /// Owns the tray icon and menu item IDs. Must be kept alive for the icon to remain visible.
    pub struct TrayHandle {
        // Keep the TrayIcon alive — dropping it removes the icon.
        _icon: tray_icon::TrayIcon,
        copy_item: MenuItem,
        copy_id: tray_icon::menu::MenuId,
        disconnect_id: tray_icon::menu::MenuId,
        quit_id: tray_icon::menu::MenuId,
        invite_url: Option<String>,
        grant_item: MenuItem,
        grant_id: tray_icon::menu::MenuId,
        revoke_item: MenuItem,
        revoke_id: tray_icon::menu::MenuId,
    }

    impl TrayHandle {
        /// Create the tray icon. `invite_url` is shown in the "Copy Invite Link" item tooltip.
        /// Pass `None` when hosting hasn't started yet; call [`set_invite_url`] later.
        ///
        /// Must be called from the main thread.
        pub fn new(invite_url: Option<String>) -> anyhow::Result<Self> {
            // macOS requires the process to register as a GUI app before tray
            // icons become visible. Without this, a CLI binary's tray icon is
            // created but never shown.
            #[cfg(target_os = "macos")]
            {
                use objc2_foundation::MainThreadMarker;
                if let Some(mtm) = MainThreadMarker::new() {
                    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
                    let app = NSApplication::sharedApplication(mtm);
                    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
                    debug!("macOS activation policy set to Accessory");
                }
            }

            let copy_item = MenuItem::new("Copy Invite Link", invite_url.is_some(), None);
            let grant_item = MenuItem::new("Allow Remote Control", true, None);
            let revoke_item = MenuItem::new("Revoke Control", false, None);
            let disconnect_item = MenuItem::new("Disconnect", true, None);
            let separator = PredefinedMenuItem::separator();
            let quit_item = MenuItem::new("Quit pairwire", true, None);

            let copy_id = copy_item.id().clone();
            let grant_id = grant_item.id().clone();
            let revoke_id = revoke_item.id().clone();
            let disconnect_id = disconnect_item.id().clone();
            let quit_id = quit_item.id().clone();

            let menu = Menu::new();
            menu.append_items(&[
                &copy_item,
                &grant_item,
                &revoke_item,
                &disconnect_item,
                &separator,
                &quit_item,
            ])?;

            // 16×16 RGBA checkerboard as a minimal placeholder icon.
            // In a real build this would be replaced with a proper PNG asset.
            let icon_rgba = make_placeholder_icon(16, 16);
            let icon = tray_icon::Icon::from_rgba(icon_rgba, 16, 16)?;

            let tray = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("pairwire")
                .with_icon(icon)
                .build()?;

            debug!("tray icon created");

            Ok(Self {
                _icon: tray,
                copy_item,
                copy_id,
                disconnect_id,
                quit_id,
                invite_url,
                grant_item,
                grant_id,
                revoke_item,
                revoke_id,
            })
        }

        /// Update the invite URL and re-enable the "Copy Invite Link" item.
        pub fn set_invite_url(&mut self, url: String) {
            self.invite_url = Some(url);
            self.copy_item.set_enabled(true);
        }

        /// Toggle the Grant / Revoke menu items based on whether control is granted.
        ///
        /// When `granted` is `true`, "Allow Remote Control" is disabled and
        /// "Revoke Control" is enabled, and vice versa.
        pub fn set_control_granted(&mut self, granted: bool) {
            self.grant_item.set_enabled(!granted);
            self.revoke_item.set_enabled(granted);
        }

        /// Poll for a pending menu event. Returns `None` if the queue is empty.
        ///
        /// Call this from the main thread event loop (e.g., inside `eframe::App::update`
        /// or a `std::thread::sleep` loop when running headless).
        pub fn poll(&self) -> Option<TrayAction> {
            match MenuEvent::receiver().try_recv() {
                Ok(event) => {
                    let id = &event.id;
                    if id == &self.quit_id {
                        debug!("tray: Quit");
                        Some(TrayAction::Quit)
                    } else if id == &self.disconnect_id {
                        debug!("tray: Disconnect");
                        Some(TrayAction::Disconnect)
                    } else if id == &self.copy_id {
                        if let Some(url) = &self.invite_url {
                            debug!("tray: CopyInvite");
                            Some(TrayAction::CopyInvite(url.clone()))
                        } else {
                            warn!("tray: CopyInvite clicked but no invite URL set");
                            None
                        }
                    } else if id == &self.grant_id {
                        debug!("tray: GrantControl");
                        Some(TrayAction::GrantControl)
                    } else if id == &self.revoke_id {
                        debug!("tray: RevokeControl");
                        Some(TrayAction::RevokeControl)
                    } else {
                        None
                    }
                }
                Err(err) => {
                    if err.is_disconnected() {
                        warn!("tray menu event channel disconnected");
                    }
                    None
                }
            }
        }
    }

    /// Generate a simple 16×16 RGBA icon: dark background with a white dot in the centre.
    fn make_placeholder_icon(w: u32, h: u32) -> Vec<u8> {
        let cx = w / 2;
        let cy = h / 2;
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                #[allow(clippy::cast_possible_wrap)]
                let dist_sq = (x as i32 - cx as i32).pow(2) + (y as i32 - cy as i32).pow(2);
                if dist_sq <= 9 {
                    // White dot
                    buf.extend_from_slice(&[255, 255, 255, 255]);
                } else {
                    // Dark background
                    buf.extend_from_slice(&[30, 30, 30, 220]);
                }
            }
        }
        buf
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn placeholder_icon_correct_size() {
            let buf = make_placeholder_icon(16, 16);
            assert_eq!(buf.len(), 16 * 16 * 4);
        }

        #[test]
        fn placeholder_icon_centre_is_white() {
            let buf = make_placeholder_icon(16, 16);
            // Centre pixel at (8, 8) → index (8 * 16 + 8) * 4 = 544
            let idx = (8 * 16 + 8) * 4;
            assert_eq!(&buf[idx..idx + 4], &[255, 255, 255, 255]);
        }
    }
}

#[cfg(feature = "tray")]
pub use inner::{TrayAction, TrayHandle};
