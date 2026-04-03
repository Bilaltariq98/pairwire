//! Drawing and annotation protocol types.
//!
//! Two drawing modes: laser pointer (fading trail) and persistent strokes.
//! Real-time preview via datagrams (`DrawPoint`), final stroke via reliable stream.

use serde::{Deserialize, Serialize};

/// Drawing mode for a stroke.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DrawMode {
    /// Trail fades after ~2 seconds. For "look here" gestures.
    Laser,
    /// Strokes persist until explicitly cleared.
    Persistent,
}

/// A single point in a drawing stroke, sent via datagram for real-time preview.
///
/// Must be < 20 bytes serialized for QUIC datagram budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrawPoint {
    /// Session-local peer ID (who is drawing).
    pub peer_id: u8,
    /// X position in host display coordinates.
    pub x: u16,
    /// Y position in host display coordinates.
    pub y: u16,
}

/// A complete drawing stroke, sent on a reliable stream after the user lifts the pen.
///
/// Contains the full point list for accurate rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrawStroke {
    /// Session-local peer ID (who drew this).
    pub peer_id: u8,
    /// Stroke color as RGB.
    pub color: [u8; 3],
    /// Ordered list of points in the stroke.
    pub points: Vec<(u16, u16)>,
    /// Drawing mode.
    pub mode: DrawMode,
}

/// Clear command — erase strokes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrawClear {
    /// Peer whose strokes to clear. `None` = clear all.
    pub peer_id: Option<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use postcard::{from_bytes, to_allocvec};

    #[test]
    fn draw_point_round_trip() {
        let pt = DrawPoint {
            peer_id: 1,
            x: 500,
            y: 300,
        };
        let bytes = to_allocvec(&pt).unwrap();
        assert_eq!(pt, from_bytes(&bytes).unwrap());
    }

    #[test]
    fn draw_point_size_under_20_bytes() {
        let pt = DrawPoint {
            peer_id: u8::MAX,
            x: u16::MAX,
            y: u16::MAX,
        };
        let bytes = to_allocvec(&pt).unwrap();
        assert!(
            bytes.len() < 20,
            "DrawPoint serialized to {} bytes, must be < 20",
            bytes.len()
        );
    }

    #[test]
    fn draw_stroke_round_trip() {
        let stroke = DrawStroke {
            peer_id: 0,
            color: [255, 0, 0],
            points: vec![(100, 200), (150, 250), (200, 300)],
            mode: DrawMode::Laser,
        };
        let bytes = to_allocvec(&stroke).unwrap();
        assert_eq!(stroke, from_bytes(&bytes).unwrap());
    }

    #[test]
    fn draw_stroke_empty_points_round_trip() {
        let stroke = DrawStroke {
            peer_id: 1,
            color: [0, 0, 255],
            points: vec![],
            mode: DrawMode::Persistent,
        };
        let bytes = to_allocvec(&stroke).unwrap();
        assert_eq!(stroke, from_bytes(&bytes).unwrap());
    }

    #[test]
    fn draw_clear_round_trip() {
        let clear = DrawClear {
            peer_id: Some(1),
        };
        let bytes = to_allocvec(&clear).unwrap();
        assert_eq!(clear, from_bytes(&bytes).unwrap());

        let clear_all = DrawClear { peer_id: None };
        let bytes = to_allocvec(&clear_all).unwrap();
        assert_eq!(clear_all, from_bytes(&bytes).unwrap());
    }
}
