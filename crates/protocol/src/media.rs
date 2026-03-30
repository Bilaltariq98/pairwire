//! Media frame headers for screen and audio tracks.

use serde::{Deserialize, Serialize};

/// Header prepended to each screen frame on the QUIC stream.
///
/// Layout (8 bytes fixed):
///   - timestamp: u48 (6 bytes, microseconds, little-endian)
///   - flags: u8
///   - reserved: u8
///
/// The `postcard` encoding of this struct may differ from the raw layout above.
/// Use `ScreenFrameHeader::to_bytes()` / `from_bytes()` for the canonical wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenFrameHeader {
    /// Frame capture timestamp in microseconds (monotonic).
    pub timestamp_us: u64,
    /// Frame flags.
    pub flags: FrameFlags,
}

/// Bitmask flags for a screen frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameFlags {
    /// This frame is an IDR/keyframe — starts a new decodable group.
    pub keyframe: bool,
    /// This is the last frame in the current group.
    pub last_in_group: bool,
}

impl ScreenFrameHeader {
    #[must_use]
    pub fn new_keyframe(timestamp_us: u64) -> Self {
        Self {
            timestamp_us,
            flags: FrameFlags {
                keyframe: true,
                last_in_group: false,
            },
        }
    }

    #[must_use]
    pub fn new_delta(timestamp_us: u64) -> Self {
        Self {
            timestamp_us,
            flags: FrameFlags {
                keyframe: false,
                last_in_group: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postcard::{from_bytes, to_allocvec};

    #[test]
    fn screen_frame_header_round_trip() {
        let hdr = ScreenFrameHeader::new_keyframe(1_000_000);
        let bytes = to_allocvec(&hdr).unwrap();
        assert_eq!(hdr, from_bytes(&bytes).unwrap());
    }

    #[test]
    fn frame_header_size_reasonable() {
        let hdr = ScreenFrameHeader::new_delta(u64::MAX);
        let bytes = to_allocvec(&hdr).unwrap();
        // Should be well under 20 bytes
        assert!(
            bytes.len() < 20,
            "ScreenFrameHeader is {} bytes",
            bytes.len()
        );
    }
}
