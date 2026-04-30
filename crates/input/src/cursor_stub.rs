//! Fallback cursor polling for unsupported platforms.

use pairwire_core::{Error, Result};

pub fn main_display_physical_size() -> (u32, u32) {
    (1920, 1080)
}

/// Cursor polling is not implemented on this platform yet.
pub fn current_cursor_position() -> Result<(u16, u16)> {
    Err(Error::Protocol(
        "cursor polling not implemented for this platform".into(),
    ))
}

/// Cursor polling is not implemented on this platform yet.
pub fn current_cursor_state() -> Result<((u16, u16), (u16, u16))> {
    Err(Error::Protocol(
        "cursor polling not implemented for this platform".into(),
    ))
}
