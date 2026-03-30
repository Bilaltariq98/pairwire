//! Stub input injection for non-macOS platforms.
//!
//! Windows (`SendInput`) and Linux (uinput/XTest) injection is planned for Phase 3.

use pairwire_core::Result;
use pairwire_protocol::input::InputMessage;
use tracing::warn;

use crate::InputPermission;

#[allow(clippy::unnecessary_wraps)] // must match inject_macos signature which can fail
pub fn inject(event: &InputMessage, _permission: InputPermission) -> Result<()> {
    warn!(
        event_type = match event {
            InputMessage::Key(_) => "key",
            InputMessage::Click(_) => "click",
            InputMessage::Scroll(_) => "scroll",
            InputMessage::Clipboard(_) => "clipboard",
        },
        "input injection not implemented on this platform"
    );
    Ok(())
}
