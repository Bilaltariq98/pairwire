//! Remote control input events: keyboard, mouse, clipboard.
//!
//! Sent on a reliable unidirectional QUIC stream (priority 1).
//! Events must not be dropped — use a reliable stream, not datagrams.

use serde::{Deserialize, Serialize};

/// Modifier key bitmask constants for `KeyEvent::modifiers`.
pub const MOD_SHIFT: u8 = 0x01;
pub const MOD_CTRL: u8 = 0x02;
pub const MOD_ALT: u8 = 0x04;
pub const MOD_META: u8 = 0x08;

/// A keyboard key press or release event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEvent {
    /// Platform-independent virtual key code.
    pub key_code: u32,
    /// Modifier bitmask (see `MOD_SHIFT`, `MOD_CTRL`, `MOD_ALT`, `MOD_META`).
    pub modifiers: u8,
    /// true = key down, false = key up.
    pub pressed: bool,
    /// Monotonic timestamp in milliseconds.
    pub timestamp_ms: u64,
}

/// A mouse button press or release event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseClick {
    /// X position in host display pixels.
    pub x: u16,
    /// Y position in host display pixels.
    pub y: u16,
    /// Button: 0=left, 1=right, 2=middle.
    pub button: u8,
    /// true = button down, false = button up.
    pub pressed: bool,
    /// Monotonic timestamp in milliseconds.
    pub timestamp_ms: u64,
}

/// A mouse move (or drag) event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseMove {
    /// X position in host display pixels.
    pub x: u16,
    /// Y position in host display pixels.
    pub y: u16,
    /// Bitmask of buttons currently held (0x01=left, 0x02=right, 0x04=middle).
    /// Used to decide between `MouseMoved` and `*MouseDragged` on the host.
    pub buttons: u8,
    /// Monotonic timestamp in milliseconds.
    pub timestamp_ms: u64,
}

/// A mouse scroll wheel event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseScroll {
    /// X position in host display pixels.
    pub x: u16,
    /// Y position in host display pixels.
    pub y: u16,
    /// Horizontal scroll delta * 100 (fixed-point).
    pub delta_x: i16,
    /// Vertical scroll delta * 100 (fixed-point).
    pub delta_y: i16,
    /// Monotonic timestamp in milliseconds.
    pub timestamp_ms: u64,
}

/// Clipboard content format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardFormat {
    PlainText,
    Html,
    Image,
    /// File paths (not file contents).
    FilePaths,
}

/// Clipboard synchronization payload.
///
/// Size-limited: reject or chunk payloads > 1MB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardSync {
    pub format: ClipboardFormat,
    pub data: Vec<u8>,
}

/// Envelope for all input stream messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputMessage {
    Key(KeyEvent),
    Click(MouseClick),
    Scroll(MouseScroll),
    Clipboard(ClipboardSync),
    Move(MouseMove),
}

#[cfg(test)]
mod tests {
    use super::*;
    use postcard::{from_bytes, to_allocvec};

    #[test]
    fn key_event_round_trip() {
        let ev = KeyEvent {
            key_code: 65,
            modifiers: 0b0001,
            pressed: true,
            timestamp_ms: 12345,
        };
        let bytes = to_allocvec(&ev).unwrap();
        assert_eq!(ev, from_bytes(&bytes).unwrap());
    }

    #[test]
    fn mouse_click_round_trip() {
        let ev = MouseClick {
            x: 800,
            y: 600,
            button: 0,
            pressed: true,
            timestamp_ms: 99999,
        };
        let bytes = to_allocvec(&ev).unwrap();
        assert_eq!(ev, from_bytes(&bytes).unwrap());
    }

    #[test]
    fn mouse_scroll_round_trip() {
        let ev = MouseScroll {
            x: 400,
            y: 300,
            delta_x: 0,
            delta_y: -300,
            timestamp_ms: 55555,
        };
        let bytes = to_allocvec(&ev).unwrap();
        assert_eq!(ev, from_bytes(&bytes).unwrap());
    }

    #[test]
    fn clipboard_round_trip() {
        let ev = ClipboardSync {
            format: ClipboardFormat::PlainText,
            data: b"hello world".to_vec(),
        };
        let bytes = to_allocvec(&ev).unwrap();
        assert_eq!(ev, from_bytes(&bytes).unwrap());
    }

    #[test]
    fn input_message_envelope_round_trip() {
        let msg = InputMessage::Key(KeyEvent {
            key_code: 13,
            modifiers: 0,
            pressed: false,
            timestamp_ms: 0,
        });
        let bytes = to_allocvec(&msg).unwrap();
        assert_eq!(msg, from_bytes(&bytes).unwrap());
    }
}
