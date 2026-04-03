//! Frame types passed between capture, encode, decode, and render threads.

use bytes::Bytes;

/// A raw uncompressed frame from the screen capturer.
#[derive(Debug, Clone)]
pub struct RawFrame {
    /// Pixel data in BGRA format (4 bytes per pixel).
    pub data: Bytes,
    pub width: u32,
    pub height: u32,
    /// Capture timestamp in microseconds (monotonic).
    pub timestamp_us: u64,
}

impl RawFrame {
    /// Expected byte length for a BGRA frame of given dimensions.
    #[must_use]
    pub fn expected_len(width: u32, height: u32) -> usize {
        (width as usize) * (height as usize) * 4
    }
}

/// An H.264-encoded packet (one or more NAL units).
#[derive(Debug, Clone)]
pub struct EncodedPacket {
    /// NAL unit data. Empty when `is_keepalive` is true.
    pub data: Bytes,
    /// Presentation timestamp in microseconds.
    pub timestamp_us: u64,
    /// Whether this packet starts a new keyframe group (IDR).
    pub is_keyframe: bool,
    /// Keepalive signal: content is static, no new frame to decode.
    ///
    /// Sent after `CaptureConfig::keepalive_after_frames` consecutive static frames.
    /// The receiver should not pass this to the decoder but should reset its
    /// inactivity timer to avoid displaying a stale "connection lost" indicator.
    pub is_keepalive: bool,
}

/// A decoded frame ready for GPU upload and display.
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// Pixel data (format depends on decoder output, typically YUV420 or BGRA).
    pub data: Bytes,
    pub width: u32,
    pub height: u32,
    /// Presentation timestamp in microseconds.
    pub timestamp_us: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_frame_expected_len() {
        assert_eq!(RawFrame::expected_len(1920, 1080), 1920 * 1080 * 4);
        assert_eq!(RawFrame::expected_len(1280, 720), 1280 * 720 * 4);
    }

    #[test]
    fn raw_frame_expected_len_zero() {
        assert_eq!(RawFrame::expected_len(0, 0), 0);
        assert_eq!(RawFrame::expected_len(100, 0), 0);
        assert_eq!(RawFrame::expected_len(0, 100), 0);
    }

    #[test]
    fn encoded_packet_keepalive() {
        let pkt = EncodedPacket {
            data: Bytes::new(),
            timestamp_us: 1000,
            is_keyframe: false,
            is_keepalive: true,
        };
        assert!(pkt.is_keepalive);
        assert!(pkt.data.is_empty());
    }

    #[test]
    fn decoded_frame_data_size() {
        let frame = DecodedFrame {
            data: Bytes::from(vec![0u8; 1920 * 1080 * 4]),
            width: 1920,
            height: 1080,
            timestamp_us: 0,
        };
        assert_eq!(
            frame.data.len(),
            RawFrame::expected_len(frame.width, frame.height)
        );
    }
}
