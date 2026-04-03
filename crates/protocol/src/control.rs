//! Control stream messages: session management, handshake, heartbeat.

use serde::{Deserialize, Serialize};

/// Sent by both peers immediately after connection to negotiate session parameters.
///
/// Version mismatch (incompatible `version` field) causes immediate disconnect.
/// Session token mismatch causes immediate disconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handshake {
    /// Protocol version. Must match `PROTOCOL_VERSION` on both sides.
    pub version: u16,
    /// Capabilities this peer supports.
    pub capabilities: Vec<Capability>,
    /// Display resolution of the host (pixels).
    pub display_width: u16,
    pub display_height: u16,
    /// Display scale factor * 100 (e.g., 200 = 2.0x Retina).
    pub display_scale: u16,
    /// Video codecs supported, in preference order.
    pub video_codecs: Vec<VideoCodec>,
    /// Audio codecs supported, in preference order.
    pub audio_codecs: Vec<AudioCodec>,
    /// Session token from the invite link. Host verifies this matches.
    /// 16 bytes, matches `SessionId`.
    pub session_token: [u8; 16],
}

/// A capability a peer may advertise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    Screen,
    Audio,
    RemoteControl,
    Clipboard,
    Annotations,
    Terminal,
}

/// Supported video codecs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodec {
    H264,
    H265,
    Av1,
}

/// Supported audio codecs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioCodec {
    Opus,
}

/// Permissions granted to a peer for remote control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permissions {
    ViewOnly,
    FullControl,
    KeyboardOnly,
}

/// Grants or revokes remote control to a specific peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlHandoff {
    /// Short session-local peer ID (index, not full `PeerId`).
    pub to_peer: u16,
    pub permissions: Permissions,
    /// If false, this is a revocation.
    pub granted: bool,
}

/// Session management messages sent on the control stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlMessage {
    Handshake(Handshake),
    ControlHandoff(ControlHandoff),
    /// Peer is disconnecting cleanly.
    Goodbye {
        reason: String,
    },
    /// Request the host to start/stop sharing.
    SetSharing {
        screen: bool,
        audio: bool,
    },
}

/// Diagnostic heartbeat sent periodically on the control stream.
///
/// Used for RTT measurement and connection health monitoring.
/// Serialized size must stay under 50 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heartbeat {
    /// Monotonic timestamp in milliseconds (wraps at `u32::MAX` ~= 49 days).
    pub timestamp_ms: u32,
    /// Round-trip time in microseconds (from previous heartbeat echo).
    pub rtt_us: u32,
    /// Packet loss rate * 1000 (e.g., 5000 = 5.0%).
    pub loss_per_mille: u16,
    /// Current video bitrate in kbps.
    pub video_kbps: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use postcard::to_allocvec;

    #[test]
    fn heartbeat_size() {
        let hb = Heartbeat {
            timestamp_ms: u32::MAX,
            rtt_us: u32::MAX,
            loss_per_mille: u16::MAX,
            video_kbps: u16::MAX,
        };
        let bytes = to_allocvec(&hb).unwrap();
        assert!(
            bytes.len() < 50,
            "Heartbeat serialized to {} bytes, must be < 50",
            bytes.len()
        );
    }

    #[test]
    fn handshake_round_trip() {
        let hs = Handshake {
            version: 1,
            capabilities: vec![Capability::Screen, Capability::Audio],
            display_width: 2560,
            display_height: 1440,
            display_scale: 200,
            video_codecs: vec![VideoCodec::H264],
            audio_codecs: vec![AudioCodec::Opus],
            session_token: [1; 16],
        };
        let bytes = to_allocvec(&hs).unwrap();
        let decoded: Handshake = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(hs, decoded);
    }
}
