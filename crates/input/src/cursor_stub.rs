//! Fallback cursor polling for unsupported platforms.

use pairwire_core::{Error, Result};

/// Cursor polling is not implemented on this platform yet.
pub fn current_cursor_position() -> Result<(u16, u16)> {
    Err(Error::Protocol(
        "cursor polling not implemented for this platform".into(),
    ))
}
