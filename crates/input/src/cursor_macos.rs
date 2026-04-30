//! macOS cursor position polling via `CGEvent` and display resolution detection.

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

/// Return the physical pixel dimensions of the main display.
///
/// On Retina displays this is the native resolution (e.g. 3024×1964 on a 14" MBP),
/// not the logical resolution. Use this to configure the capture pipeline so no
/// unnecessary scaling is applied before encoding.
///
/// Falls back to `(1920, 1080)` if the display cannot be queried.
pub fn main_display_physical_size() -> (u32, u32) {
    let display = CGDisplay::main();
    let pixels_wide = display.pixels_wide();
    let pixels_high = display.pixels_high();
    if pixels_wide == 0 || pixels_high == 0 {
        (1920, 1080)
    } else {
        #[allow(clippy::cast_possible_truncation)]
        (pixels_wide as u32, pixels_high as u32)
    }
}
