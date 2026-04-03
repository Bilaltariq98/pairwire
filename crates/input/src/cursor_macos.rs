//! macOS cursor position polling via `CGEvent`.

use core_graphics::{
    event::CGEvent,
    event_source::{CGEventSource, CGEventSourceStateID},
};
use pairwire_core::{Error, Result};

/// Read the current global cursor position in display coordinates.
pub fn current_cursor_position() -> Result<(u16, u16)> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|()| {
        Error::PermissionDenied("CGEventSource creation failed for cursor polling".into())
    })?;
    let event = CGEvent::new(source)
        .map_err(|()| Error::PermissionDenied("CGEvent creation failed".into()))?;
    let location = event.location();

    let x = location.x.clamp(0.0, f64::from(u16::MAX)) as u16;
    let y = location.y.clamp(0.0, f64::from(u16::MAX)) as u16;

    Ok((x, y))
}
