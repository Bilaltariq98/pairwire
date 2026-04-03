//! macOS cursor position polling via `CGEvent`.

use core_graphics::{
    display::CGDisplay,
    event::CGEvent,
    event_source::{CGEventSource, CGEventSourceStateID},
};
use pairwire_core::{Error, Result};

/// Read the current global cursor position in display coordinates.
pub fn current_cursor_position() -> Result<(u16, u16)> {
    let ((x, y), _) = current_cursor_state()?;
    Ok((x, y))
}

/// Read the current global cursor position and main display size in pixels.
pub fn current_cursor_state() -> Result<((u16, u16), (u16, u16))> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|()| {
        Error::PermissionDenied("CGEventSource creation failed for cursor polling".into())
    })?;
    let event = CGEvent::new(source)
        .map_err(|()| Error::PermissionDenied("CGEvent creation failed".into()))?;
    let location = event.location();
    let bounds = CGDisplay::main().bounds();

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (x, y, width, height) = (
        location.x.clamp(0.0, f64::from(u16::MAX)) as u16,
        location.y.clamp(0.0, f64::from(u16::MAX)) as u16,
        bounds.size.width.clamp(0.0, f64::from(u16::MAX)) as u16,
        bounds.size.height.clamp(0.0, f64::from(u16::MAX)) as u16,
    );

    Ok(((x, y), (width, height)))
}
