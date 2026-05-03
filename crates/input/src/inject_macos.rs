//! macOS input injection via `CGEvent`.
//!
//! Requires the Accessibility permission (System Settings → Privacy → Accessibility).
//! Without it, `CGEvent` posting silently fails.
//!
//! Key code mapping: pairwire uses USB HID key codes (same as `CGKeyCode` on macOS).
//! Mouse coordinates are in global screen space (pixels from top-left).

use core_graphics::{
    event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, ScrollEventUnit,
    },
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGPoint,
};
use pairwire_core::{Error, Result};
use pairwire_protocol::input::{InputMessage, KeyEvent, MouseClick, MouseMove, MouseScroll};

use crate::InputPermission;

pub fn inject(event: &InputMessage, permission: InputPermission) -> Result<()> {
    match event {
        InputMessage::Key(e) => inject_key(e),
        InputMessage::Click(e) => {
            if permission == InputPermission::KeyboardOnly {
                return Ok(()); // mouse blocked by permission
            }
            inject_click(e)
        }
        InputMessage::Scroll(e) => {
            if permission == InputPermission::KeyboardOnly {
                return Ok(());
            }
            inject_scroll(e)
        }
        InputMessage::Move(e) => {
            if permission == InputPermission::KeyboardOnly {
                return Ok(());
            }
            inject_move(e)
        }
        // Clipboard is handled separately via the clipboard sync path.
        InputMessage::Clipboard(_) => Ok(()),
    }
}

fn event_source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|()| {
        Error::PermissionDenied(
            "CGEventSource creation failed — check Accessibility permission".into(),
        )
    })
}

fn inject_key(e: &KeyEvent) -> Result<()> {
    let source = event_source()?;
    let key_code = u16::try_from(e.key_code).unwrap_or(u16::MAX);
    let key_down = e.pressed;

    let event = CGEvent::new_keyboard_event(source, key_code, key_down)
        .map_err(|()| Error::PermissionDenied("CGEvent keyboard creation failed".into()))?;

    // Apply modifier flags
    let mut flags = CGEventFlags::empty();
    if e.modifiers & 0b0001 != 0 {
        flags |= CGEventFlags::CGEventFlagShift;
    }
    if e.modifiers & 0b0010 != 0 {
        flags |= CGEventFlags::CGEventFlagControl;
    }
    if e.modifiers & 0b0100 != 0 {
        flags |= CGEventFlags::CGEventFlagAlternate;
    }
    if e.modifiers & 0b1000 != 0 {
        flags |= CGEventFlags::CGEventFlagCommand;
    }
    event.set_flags(flags);
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn inject_click(e: &MouseClick) -> Result<()> {
    let source = event_source()?;
    let pos = CGPoint::new(f64::from(e.x), f64::from(e.y));

    let (down_type, up_type, button) = match e.button {
        0 => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
        ),
        1 => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGMouseButton::Right,
        ),
        _ => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGMouseButton::Center,
        ),
    };

    let event_type = if e.pressed { down_type } else { up_type };
    let event = CGEvent::new_mouse_event(source, event_type, pos, button)
        .map_err(|()| Error::PermissionDenied("CGEvent mouse creation failed".into()))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn inject_move(e: &MouseMove) -> Result<()> {
    let source = event_source()?;
    let pos = CGPoint::new(f64::from(e.x), f64::from(e.y));

    let (event_type, button) = if e.buttons & 0x01 != 0 {
        (CGEventType::LeftMouseDragged, CGMouseButton::Left)
    } else if e.buttons & 0x02 != 0 {
        (CGEventType::RightMouseDragged, CGMouseButton::Right)
    } else if e.buttons & 0x04 != 0 {
        (CGEventType::OtherMouseDragged, CGMouseButton::Center)
    } else {
        (CGEventType::MouseMoved, CGMouseButton::Left)
    };

    let event = CGEvent::new_mouse_event(source, event_type, pos, button)
        .map_err(|()| Error::PermissionDenied("CGEvent mouse move creation failed".into()))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn inject_scroll(e: &MouseScroll) -> Result<()> {
    let source = event_source()?;
    // CGEvent scroll takes integer wheel deltas; our wire format is fixed-point *100.
    let delta_y = i32::from(e.delta_y / 100);
    let delta_x = i32::from(e.delta_x / 100);

    let event = CGEvent::new_scroll_event(source, ScrollEventUnit::PIXEL, 2, delta_y, delta_x, 0)
        .map_err(|()| Error::PermissionDenied("CGEvent scroll creation failed".into()))?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}
