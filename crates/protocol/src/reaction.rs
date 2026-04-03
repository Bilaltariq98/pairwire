//! Emoji reactions sent via QUIC datagrams.
//!
//! Reactions appear at the sender's cursor position and float upward with fade.
//! Must serialize to < 15 bytes for QUIC datagram budget.

use serde::{Deserialize, Serialize};

/// Predefined reaction emojis, keyed by index.
pub const EMOJI_SET: [&str; 8] = ["👍", "👎", "❤️", "🎉", "❓", "🐛", "💡", "🔥"];

/// A quick emoji reaction at the sender's cursor position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaction {
    /// Session-local peer ID (who sent the reaction).
    pub peer_id: u8,
    /// Index into `EMOJI_SET` (0–7).
    pub emoji_id: u8,
    /// X position in host display coordinates.
    pub x: u16,
    /// Y position in host display coordinates.
    pub y: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use postcard::{from_bytes, to_allocvec};

    #[test]
    fn reaction_round_trip() {
        let r = Reaction {
            peer_id: 0,
            emoji_id: 3,
            x: 960,
            y: 540,
        };
        let bytes = to_allocvec(&r).unwrap();
        assert_eq!(r, from_bytes(&bytes).unwrap());
    }

    #[test]
    fn reaction_size_under_15_bytes() {
        let r = Reaction {
            peer_id: u8::MAX,
            emoji_id: u8::MAX,
            x: u16::MAX,
            y: u16::MAX,
        };
        let bytes = to_allocvec(&r).unwrap();
        assert!(
            bytes.len() < 15,
            "Reaction serialized to {} bytes, must be < 15",
            bytes.len()
        );
    }

    #[test]
    fn reaction_max_values_round_trip() {
        let r = Reaction {
            peer_id: u8::MAX,
            emoji_id: 7,
            x: u16::MAX,
            y: u16::MAX,
        };
        let bytes = to_allocvec(&r).unwrap();
        assert_eq!(r, from_bytes(&bytes).unwrap());
    }

    #[test]
    fn emoji_set_has_8_entries() {
        assert_eq!(EMOJI_SET.len(), 8);
    }
}
