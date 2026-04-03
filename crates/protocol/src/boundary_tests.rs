//! Adversarial and boundary-value tests for all protocol types.
//!
//! Verifies behavior with MAX values, empty collections, truncated input,
//! and zero-length payloads. Ensures the protocol layer handles edge cases
//! without panicking.

#[cfg(test)]
mod tests {
    use postcard::{from_bytes, to_allocvec};

    use crate::control::*;
    use crate::cursor::CursorUpdate;
    use crate::input::*;

    // === CursorUpdate boundary ===

    #[test]
    fn cursor_update_max_values_round_trip() {
        let cu = CursorUpdate {
            peer_id: u16::MAX,
            x: u16::MAX,
            y: u16::MAX,
            visible: true,
            click_animation: u8::MAX,
        };
        let bytes = to_allocvec(&cu).unwrap();
        let decoded: CursorUpdate = from_bytes(&bytes).unwrap();
        assert_eq!(cu, decoded);
    }

    #[test]
    fn cursor_update_all_zeroes_round_trip() {
        let cu = CursorUpdate {
            peer_id: 0,
            x: 0,
            y: 0,
            visible: false,
            click_animation: 0,
        };
        let bytes = to_allocvec(&cu).unwrap();
        let decoded: CursorUpdate = from_bytes(&bytes).unwrap();
        assert_eq!(cu, decoded);
    }

    #[test]
    fn cursor_update_truncated_input_fails() {
        let cu = CursorUpdate {
            peer_id: 1,
            x: 100,
            y: 200,
            visible: true,
            click_animation: 5,
        };
        let bytes = to_allocvec(&cu).unwrap();
        // Truncate to half the payload
        let truncated = &bytes[..bytes.len() / 2];
        assert!(from_bytes::<CursorUpdate>(truncated).is_err());
    }

    #[test]
    fn cursor_update_empty_input_fails() {
        assert!(from_bytes::<CursorUpdate>(&[]).is_err());
    }

    #[test]
    fn cursor_update_single_byte_fails() {
        assert!(from_bytes::<CursorUpdate>(&[0x42]).is_err());
    }

    // === Heartbeat boundary ===

    #[test]
    fn heartbeat_max_values_round_trip() {
        let hb = Heartbeat {
            timestamp_ms: u32::MAX,
            rtt_us: u32::MAX,
            loss_per_mille: u16::MAX,
            video_kbps: u16::MAX,
        };
        let bytes = to_allocvec(&hb).unwrap();
        let decoded: Heartbeat = from_bytes(&bytes).unwrap();
        assert_eq!(hb, decoded);
    }

    #[test]
    fn heartbeat_all_zeroes_round_trip() {
        let hb = Heartbeat {
            timestamp_ms: 0,
            rtt_us: 0,
            loss_per_mille: 0,
            video_kbps: 0,
        };
        let bytes = to_allocvec(&hb).unwrap();
        let decoded: Heartbeat = from_bytes(&bytes).unwrap();
        assert_eq!(hb, decoded);
    }

    #[test]
    fn heartbeat_empty_input_fails() {
        assert!(from_bytes::<Heartbeat>(&[]).is_err());
    }

    #[test]
    fn heartbeat_truncated_input_fails() {
        let hb = Heartbeat {
            timestamp_ms: 1000,
            rtt_us: 5000,
            loss_per_mille: 50,
            video_kbps: 2500,
        };
        let bytes = to_allocvec(&hb).unwrap();
        assert!(from_bytes::<Heartbeat>(&bytes[..3]).is_err());
    }

    // === Handshake boundary ===

    #[test]
    fn handshake_empty_collections_round_trip() {
        let hs = Handshake {
            version: crate::PROTOCOL_VERSION,
            capabilities: vec![],
            display_width: 0,
            display_height: 0,
            display_scale: 0,
            video_codecs: vec![],
            audio_codecs: vec![],
            session_token: [0; 16],
        };
        let bytes = to_allocvec(&hs).unwrap();
        let decoded: Handshake = from_bytes(&bytes).unwrap();
        assert_eq!(hs, decoded);
    }

    #[test]
    fn handshake_max_display_values_round_trip() {
        let hs = Handshake {
            version: u16::MAX,
            capabilities: vec![
                Capability::Screen,
                Capability::Audio,
                Capability::RemoteControl,
                Capability::Clipboard,
                Capability::Annotations,
                Capability::Terminal,
            ],
            display_width: u16::MAX,
            display_height: u16::MAX,
            display_scale: u16::MAX,
            video_codecs: vec![VideoCodec::H264, VideoCodec::H265, VideoCodec::Av1],
            audio_codecs: vec![AudioCodec::Opus],
            session_token: [0xFF; 16],
        };
        let bytes = to_allocvec(&hs).unwrap();
        let decoded: Handshake = from_bytes(&bytes).unwrap();
        assert_eq!(hs, decoded);
    }

    #[test]
    fn handshake_empty_input_fails() {
        assert!(from_bytes::<Handshake>(&[]).is_err());
    }

    #[test]
    fn handshake_truncated_input_fails() {
        let hs = Handshake {
            version: 1,
            capabilities: vec![Capability::Screen],
            display_width: 1920,
            display_height: 1080,
            display_scale: 100,
            video_codecs: vec![VideoCodec::H264],
            audio_codecs: vec![AudioCodec::Opus],
            session_token: [0xAB; 16],
        };
        let bytes = to_allocvec(&hs).unwrap();
        assert!(from_bytes::<Handshake>(&bytes[..bytes.len() / 2]).is_err());
    }

    // === KeyEvent boundary ===

    #[test]
    fn key_event_max_values_round_trip() {
        let ev = KeyEvent {
            key_code: u32::MAX,
            modifiers: u8::MAX,
            pressed: true,
            timestamp_ms: u64::MAX,
        };
        let bytes = to_allocvec(&ev).unwrap();
        let decoded: KeyEvent = from_bytes(&bytes).unwrap();
        assert_eq!(ev, decoded);
    }

    #[test]
    fn key_event_all_zeroes_round_trip() {
        let ev = KeyEvent {
            key_code: 0,
            modifiers: 0,
            pressed: false,
            timestamp_ms: 0,
        };
        let bytes = to_allocvec(&ev).unwrap();
        let decoded: KeyEvent = from_bytes(&bytes).unwrap();
        assert_eq!(ev, decoded);
    }

    #[test]
    fn key_event_empty_input_fails() {
        assert!(from_bytes::<KeyEvent>(&[]).is_err());
    }

    // === MouseClick boundary ===

    #[test]
    fn mouse_click_max_values_round_trip() {
        let ev = MouseClick {
            x: u16::MAX,
            y: u16::MAX,
            button: u8::MAX,
            pressed: true,
            timestamp_ms: u64::MAX,
        };
        let bytes = to_allocvec(&ev).unwrap();
        let decoded: MouseClick = from_bytes(&bytes).unwrap();
        assert_eq!(ev, decoded);
    }

    #[test]
    fn mouse_click_empty_input_fails() {
        assert!(from_bytes::<MouseClick>(&[]).is_err());
    }

    // === MouseScroll boundary ===

    #[test]
    fn mouse_scroll_extreme_deltas_round_trip() {
        let ev = MouseScroll {
            x: u16::MAX,
            y: u16::MAX,
            delta_x: i16::MIN,
            delta_y: i16::MAX,
            timestamp_ms: u64::MAX,
        };
        let bytes = to_allocvec(&ev).unwrap();
        let decoded: MouseScroll = from_bytes(&bytes).unwrap();
        assert_eq!(ev, decoded);
    }

    #[test]
    fn mouse_scroll_empty_input_fails() {
        assert!(from_bytes::<MouseScroll>(&[]).is_err());
    }

    // === ClipboardSync boundary ===

    #[test]
    fn clipboard_empty_data_round_trip() {
        let cs = ClipboardSync {
            format: ClipboardFormat::PlainText,
            data: vec![],
        };
        let bytes = to_allocvec(&cs).unwrap();
        let decoded: ClipboardSync = from_bytes(&bytes).unwrap();
        assert_eq!(cs, decoded);
    }

    #[test]
    fn clipboard_large_data_round_trip() {
        // 64KB payload — well within limits, tests vec serialization
        let cs = ClipboardSync {
            format: ClipboardFormat::Html,
            data: vec![0xAB; 65_536],
        };
        let bytes = to_allocvec(&cs).unwrap();
        let decoded: ClipboardSync = from_bytes(&bytes).unwrap();
        assert_eq!(cs, decoded);
    }

    #[test]
    fn clipboard_empty_input_fails() {
        assert!(from_bytes::<ClipboardSync>(&[]).is_err());
    }

    #[test]
    fn clipboard_all_formats_round_trip() {
        for format in [
            ClipboardFormat::PlainText,
            ClipboardFormat::Html,
            ClipboardFormat::Image,
            ClipboardFormat::FilePaths,
        ] {
            let cs = ClipboardSync {
                format,
                data: b"test".to_vec(),
            };
            let bytes = to_allocvec(&cs).unwrap();
            let decoded: ClipboardSync = from_bytes(&bytes).unwrap();
            assert_eq!(cs, decoded);
        }
    }

    // === ControlMessage envelope boundary ===

    #[test]
    fn control_message_goodbye_empty_reason_round_trip() {
        let msg = ControlMessage::Goodbye {
            reason: String::new(),
        };
        let bytes = to_allocvec(&msg).unwrap();
        let decoded: ControlMessage = from_bytes(&bytes).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn control_message_goodbye_long_reason_round_trip() {
        let msg = ControlMessage::Goodbye {
            reason: "x".repeat(1000),
        };
        let bytes = to_allocvec(&msg).unwrap();
        let decoded: ControlMessage = from_bytes(&bytes).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn control_message_empty_input_fails() {
        assert!(from_bytes::<ControlMessage>(&[]).is_err());
    }

    #[test]
    fn control_message_set_sharing_round_trip() {
        let msg = ControlMessage::SetSharing {
            screen: true,
            audio: false,
        };
        let bytes = to_allocvec(&msg).unwrap();
        let decoded: ControlMessage = from_bytes(&bytes).unwrap();
        assert_eq!(msg, decoded);
    }

    // === InputMessage envelope boundary ===

    #[test]
    fn input_message_all_variants_round_trip() {
        let variants: Vec<InputMessage> = vec![
            InputMessage::Key(KeyEvent {
                key_code: 0,
                modifiers: 0,
                pressed: false,
                timestamp_ms: 0,
            }),
            InputMessage::Click(MouseClick {
                x: 0,
                y: 0,
                button: 0,
                pressed: false,
                timestamp_ms: 0,
            }),
            InputMessage::Scroll(MouseScroll {
                x: 0,
                y: 0,
                delta_x: 0,
                delta_y: 0,
                timestamp_ms: 0,
            }),
            InputMessage::Clipboard(ClipboardSync {
                format: ClipboardFormat::PlainText,
                data: vec![],
            }),
        ];
        for msg in &variants {
            let bytes = to_allocvec(msg).unwrap();
            let decoded: InputMessage = from_bytes(&bytes).unwrap();
            assert_eq!(msg, &decoded);
        }
    }

    #[test]
    fn input_message_empty_input_fails() {
        assert!(from_bytes::<InputMessage>(&[]).is_err());
    }

    // === ControlHandoff boundary ===

    #[test]
    fn control_handoff_all_permissions_round_trip() {
        for perm in [
            Permissions::ViewOnly,
            Permissions::FullControl,
            Permissions::KeyboardOnly,
        ] {
            let ch = ControlHandoff {
                to_peer: u16::MAX,
                permissions: perm,
                granted: false,
            };
            let bytes = to_allocvec(&ch).unwrap();
            let decoded: ControlHandoff = from_bytes(&bytes).unwrap();
            assert_eq!(ch, decoded);
        }
    }

    // === Invalid enum discriminant ===

    #[test]
    fn invalid_control_message_discriminant_fails() {
        // ControlMessage has 4 variants (0..3). Byte 0xFF should be invalid.
        assert!(from_bytes::<ControlMessage>(&[0xFF]).is_err());
    }

    #[test]
    fn invalid_input_message_discriminant_fails() {
        // InputMessage has 4 variants (0..3). Byte 0xFF should be invalid.
        assert!(from_bytes::<InputMessage>(&[0xFF]).is_err());
    }

    #[test]
    fn invalid_clipboard_format_discriminant_fails() {
        // ClipboardFormat has 4 variants. Byte 0xFF should be invalid.
        assert!(from_bytes::<ClipboardFormat>(&[0xFF]).is_err());
    }

    #[test]
    fn invalid_capability_discriminant_fails() {
        assert!(from_bytes::<Capability>(&[0xFF]).is_err());
    }

    #[test]
    fn invalid_video_codec_discriminant_fails() {
        assert!(from_bytes::<VideoCodec>(&[0xFF]).is_err());
    }

    // === Garbage data ===

    #[test]
    fn garbage_bytes_rejected() {
        let garbage = [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
        // These may or may not parse depending on the type, but must not panic
        let _ = from_bytes::<CursorUpdate>(&garbage);
        let _ = from_bytes::<Heartbeat>(&garbage);
        let _ = from_bytes::<Handshake>(&garbage);
        let _ = from_bytes::<KeyEvent>(&garbage);
        let _ = from_bytes::<ControlMessage>(&garbage);
        let _ = from_bytes::<InputMessage>(&garbage);
    }
}
