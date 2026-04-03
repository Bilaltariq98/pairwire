//! Keyboard, mouse, and clipboard input forwarding.
//!
//! Two roles:
//! - `InputSender` (guest side): receives `InputMessage` events from a channel,
//!   serializes them, and sends on a reliable QUIC unidirectional stream.
//! - `InputReceiver` (host side): reads `InputMessage` events from a QUIC stream,
//!   checks permissions, and injects them into the OS.
//!
//! Permission model: the host must grant `ControlHandoff` before any events are
//! injected. Events received without permission are silently dropped.
//!
//! Platform injection:
//! - macOS: `CGEvent` posting via `core-graphics` (requires Accessibility permission)
//! - Other platforms: logged as unimplemented (Phase 3)

pub mod clipboard;
pub mod receiver;
pub mod sender;

#[cfg(target_os = "macos")]
mod cursor_macos;
#[cfg(not(target_os = "macos"))]
mod cursor_stub;

#[cfg(target_os = "macos")]
mod inject_macos;

#[cfg(not(target_os = "macos"))]
mod inject_stub;

pub use clipboard::{
    ClipboardReceiver, ClipboardReceiverHandle, ClipboardSender, ClipboardSenderHandle,
};
#[cfg(target_os = "macos")]
pub use cursor_macos::current_cursor_position;
#[cfg(not(target_os = "macos"))]
pub use cursor_stub::current_cursor_position;
pub use receiver::{InputReceiver, InputReceiverHandle};
pub use sender::{InputSender, InputSenderHandle};

/// Permission state for input injection on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputPermission {
    /// No input events will be injected.
    #[default]
    Denied,
    /// All keyboard and mouse events are injected.
    FullControl,
    /// Only keyboard events are injected.
    KeyboardOnly,
}
