# Phase 1: Proof of Concept — Screen Sharing Over iroh

> Goal: Two peers share a screen and cursor over iroh P2P. No audio, no moq-lite yet. macOS only.
> Estimated effort: 4–6 weeks

## Tasks

### P1.1 — iroh Endpoint Setup
- **Status**: ✅ Complete
- **Crate**: `crates/transport`
- **Key deliverables**: `Transport` (bind/connect/accept), `InviteLink` (Display+FromStr), `ALPN` constant
- **Notes**: iroh 0.97 API — `endpoint.id()`, `endpoint.addr()`, `endpoint.online().await`, `conn.remote_id()`. See LEARNINGS.md.

### P1.2 — Raw Stream & Datagram Helpers
- **Status**: ✅ Complete
- **Crate**: `crates/transport`
- **Key deliverables**: `PairwireConnection` wrapper, `open_control_stream()`, `open_input_stream()`, `accept_bi()`, `accept_uni()`, `send_cursor_datagram()`, `recv_cursor_datagram()`, `send_message<T>()`, `recv_message<T>()`
- **Notes**: Length-prefixed postcard framing (4-byte LE length + payload). Max message 4MB. Datagram size asserted < 1200 bytes.

### P1.3 — Screen Capture Thread
- **Status**: 🔲 Not started
- **Crate**: `crates/capture`
- **Key deliverables**: `CaptureThread`, `start() -> Receiver<RawFrame>`, `stop()`
- **Depends on**: P0.2
- **Platform**: macOS (scap + ScreenCaptureKit)

### P1.4 — FFmpeg H.264 Encode Thread
- **Status**: 🔲 Not started
- **Crate**: `crates/capture`
- **Key deliverables**: `EncodeThread`, hardware encoder selection (videotoolbox → libx264 fallback), static frame detection
- **Depends on**: P1.3

### P1.5 — FFmpeg H.264 Decode Thread
- **Status**: 🔲 Not started
- **Crate**: `crates/capture`
- **Key deliverables**: `DecodeThread`, hardware decoder selection
- **Depends on**: P1.4

### P1.6 — Network Sender (Screen Frames)
- **Status**: 🔲 Not started
- **Crate**: `crates/transport`
- **Key deliverables**: `ScreenSender`, keyframe-per-stream model, priority 3
- **Depends on**: P1.2, P1.4

### P1.7 — Network Receiver (Screen Frames)
- **Status**: 🔲 Not started
- **Crate**: `crates/transport`
- **Key deliverables**: `ScreenReceiver`, stream multiplexing, drop-old-groups
- **Depends on**: P1.6, P1.5

### P1.8 — Cursor Sharing via Datagrams
- **Status**: 🔲 Not started
- **Crate**: `crates/transport`
- **Key deliverables**: `CursorSender` (60Hz), `CursorReceiver` (atomic latest position)
- **Depends on**: P1.2

### P1.9 — Basic Viewer UI
- **Status**: 🔲 Not started
- **Crate**: `crates/app`
- **Key deliverables**: egui window, GPU texture upload (wgpu), cursor overlay, FPS counter
- **Depends on**: P1.5, P1.8

### P1.10 — End-to-End Integration
- **Status**: 🔲 Not started
- **Crate**: `crates/app`
- **Key deliverables**: `pairwire host` / `pairwire join <invite>` CLI, graceful shutdown
- **Depends on**: P1.1–P1.9
- **Performance targets**: screen latency < 100ms, cursor latency < 50ms, CPU < 15% at 1080p 30fps

## Notes

- iroh must be pinned carefully — v0.96 has a hole-punch regression (see LEARNINGS.md)
- FFmpeg encoder contexts are !Send — must stay on their dedicated thread
- scap capture callback must not be blocked — send to channel immediately
