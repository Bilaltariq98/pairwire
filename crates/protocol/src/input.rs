//! Remote control input events: keyboard, mouse, clipboard.
//!
//! Sent on a reliable unidirectional QUIC stream (priority 1).
//! Events must not be dropped — use a reliable stream, not datagrams.

use serde::{Deserialize, Serialize};

/// A keyboard key press or release event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEvent {
    /// Platform-independent virtual key code.
    pub key_code: u32,
    /// Modifier bitmask: bit 0=Shift, 1=Ctrl, 2=Alt, 3=Meta/Cmd.
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
