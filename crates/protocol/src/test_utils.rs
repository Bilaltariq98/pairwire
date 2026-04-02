//! Test helpers for protocol message types.
//!
//! Provides factory functions for every message type and a generic round-trip assertion.

use crate::control::{
    AudioCodec, Capability, ControlHandoff, Handshake, Heartbeat, Permissions, VideoCodec,
};
use crate::cursor::CursorUpdate;
use crate::input::{ClipboardFormat, ClipboardSync, KeyEvent, MouseClick, MouseScroll};
use postcard::{from_bytes, to_allocvec};
use serde::{Serialize, de::DeserializeOwned};

/// Serialize then deserialize `value`, asserting the result equals the original.
pub fn assert_round_trip<T>(value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let bytes = to_allocvec(value).expect("serialization failed");
    let decoded: T = from_bytes(&bytes).expect("deserialization failed");
    assert_eq!(value, &decoded, "round-trip mismatch");
}

pub fn make_handshake() -> Handshake {
    Handshake {
        version: crate::PROTOCOL_VERSION,
        capabilities: vec![
            Capability::Screen,
            Capability::Audio,
            Capability::RemoteControl,
        ],
        display_width: 1920,
        display_height: 1080,
        display_scale: 100,
        video_codecs: vec![VideoCodec::H264],
        audio_codecs: vec![AudioCodec::Opus],
    }
}

pub fn make_heartbeat() -> Heartbeat {
    Heartbeat {
        timestamp_ms: 12345,
        rtt_us: 5000,
        loss_per_mille: 0,
        video_kbps: 2500,
    }
}

pub fn make_cursor_update() -> CursorUpdate {
    CursorUpdate {
        peer_id: 1,
        x: 960,
        y: 540,
        visible: true,
        click_animation: 0,
    }
}

pub fn make_key_event() -> KeyEvent {
    KeyEvent {
        key_code: 65,
        modifiers: 0,
        pressed: true,
        timestamp_ms: 1000,
    }
}

pub fn make_mouse_click() -> MouseClick {
    MouseClick {
        x: 400,
        y: 300,
        button: 0,
        pressed: true,
        timestamp_ms: 2000,
    }
}

pub fn make_mouse_scroll() -> MouseScroll {
    MouseScroll {
        x: 400,
        y: 300,
        delta_x: 0,
        delta_y: -100,
        timestamp_ms: 3000,
    }
}

pub fn make_clipboard_sync() -> ClipboardSync {
    ClipboardSync {
        format: ClipboardFormat::PlainText,
        data: b"fn main() {}".to_vec(),
    }
}

pub fn make_control_handoff() -> ControlHandoff {
    ControlHandoff {
        to_peer: 2,
        permissions: Permissions::FullControl,
        granted: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ControlMessage;
    use crate::input::InputMessage;

    #[test]
    fn all_types_round_trip() {
        assert_round_trip(&make_handshake());
        assert_round_trip(&make_heartbeat());
        assert_round_trip(&make_cursor_update());
        assert_round_trip(&make_key_event());
        assert_round_trip(&make_mouse_click());
        assert_round_trip(&make_mouse_scroll());
        assert_round_trip(&make_clipboard_sync());
        assert_round_trip(&make_control_handoff());
        assert_round_trip(&ControlMessage::Handshake(make_handshake()));
        assert_round_trip(&InputMessage::Key(make_key_event()));
    }
}
