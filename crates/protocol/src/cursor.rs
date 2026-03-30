//! Cursor presence update, sent as QUIC datagrams at up to 60Hz.
//!
//! Must serialize to < 50 bytes (QUIC datagram budget).

use serde::{Deserialize, Serialize};

/// Remote cursor position and state, sent unreliably via QUIC datagrams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorUpdate {
    /// Short session-local peer ID (u16 to save bytes).
    pub peer_id: u16,
    /// Cursor X position in pixels (host display coordinates).
    pub x: u16,
    /// Cursor Y position in pixels (host display coordinates).
    pub y: u16,
    /// Whether the cursor is visible.
    pub visible: bool,
    /// Click animation frame: 0 = none, 1–10 = ripple animation frame.
    pub click_animation: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use postcard::to_allocvec;

    #[test]
    fn cursor_update_size() {
        let cu = CursorUpdate {
            peer_id: u16::MAX,
            x: u16::MAX,
            y: u16::MAX,
            visible: true,
            click_animation: 10,
        };
        let bytes = to_allocvec(&cu).unwrap();
        assert!(
            bytes.len() < 50,
            "CursorUpdate serialized to {} bytes, must be < 50",
            bytes.len()
        );
    }

    #[test]
    fn cursor_update_round_trip() {
        let cu = CursorUpdate {
            peer_id: 1,
            x: 1280,
            y: 720,
            visible: true,
            click_animation: 0,
        };
        let bytes = to_allocvec(&cu).unwrap();
        let decoded: CursorUpdate = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(cu, decoded);
    }
}
