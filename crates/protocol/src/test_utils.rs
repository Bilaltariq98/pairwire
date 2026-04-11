//! Test helpers for protocol message types.
//!
//! Provides factory functions for every message type and a generic round-trip assertion.

use crate::control::{
    AudioCodec, Capability, ControlHandoff, ControlRequest, Handshake, Heartbeat, Permissions,
    VideoCodec,
};
use crate::cursor::CursorUpdate;
use crate::drawing::{DrawMode, DrawPoint, DrawStroke};
use crate::input::{ClipboardFormat, ClipboardSync, KeyEvent, MouseClick, MouseScroll};
use crate::reaction::Reaction;
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
        session_token: [0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
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

pub fn make_draw_point() -> DrawPoint {
    DrawPoint {
        peer_id: 1,
        x: 500,
        y: 300,
    }
}

pub fn make_draw_stroke() -> DrawStroke {
    DrawStroke {
        peer_id: 0,
        color: [255, 80, 80],
        points: vec![(100, 200), (150, 250), (200, 300)],
        mode: DrawMode::Laser,
    }
}

pub fn make_reaction() -> Reaction {
    Reaction {
        peer_id: 0,
        emoji_id: 3,
        x: 960,
        y: 540,
    }
}

pub fn make_control_request() -> ControlRequest {
    ControlRequest { peer_id: 1 }
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
        assert_round_trip(&make_draw_point());
        assert_round_trip(&make_draw_stroke());
        assert_round_trip(&make_reaction());
        assert_round_trip(&make_control_request());
        assert_round_trip(&ControlMessage::Handshake(make_handshake()));
        assert_round_trip(&ControlMessage::ControlRequest(make_control_request()));
        assert_round_trip(&InputMessage::Key(make_key_event()));
    }
}
