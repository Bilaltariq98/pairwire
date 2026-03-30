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
- **Status**: ✅ Complete
- **Crate**: `crates/capture`
- **Key deliverables**: `CaptureThread::start_fake()` (always available), `start()` (behind `capture` feature), `into_async_receiver()` bridge helper
- **Channel design**: `std::sync::mpsc` output (OS thread → OS thread, no runtime needed)
- **Tests**: fake_capture_produces_frames, fake_capture_frames_have_increasing_timestamps

### P1.4 — FFmpeg H.264 Encode Thread
- **Status**: ✅ Complete
- **Crate**: `crates/capture`
- **Key deliverables**: `EncodeThread` (behind `ffmpeg` feature), libx264 with ultrafast/zerolatency, h264_videotoolbox fallback, static frame hash detection
- **Channel design**: `std::sync::mpsc` input, `tokio::sync::mpsc` output (blocking_send safe from OS thread)
- **Key fix**: Must use `avcodec_alloc_context3(codec.as_ptr())` not null — sets codec_type correctly
- **Tests**: ffmpeg_encode_direct, encode_fake_frames_produces_packets, static_frames_are_skipped

### P1.5 — FFmpeg H.264 Decode Thread
- **Status**: ✅ Complete
- **Crate**: `crates/capture`
- **Key deliverables**: `DecodeThread` (behind `ffmpeg` feature), lazy scaler init on first frame, YUV→BGRA conversion
- **Channel design**: `std::sync::mpsc` input, `tokio::sync::mpsc` output
- **Tests**: encode_decode_round_trip (full pipeline: fake capture → encode → decode)

### P1.6 — Network Sender (Screen Frames)
- **Status**: ✅ Complete
- **Crate**: `crates/transport`
- **Key deliverables**: `ScreenSender::spawn()` — tokio task, new stream per keyframe, `SenderHandle`
- **Protocol**: `ScreenFrameHeader` (postcard, length-prefixed) + raw NAL bytes (4-byte LE length prefix)

### P1.7 — Network Receiver (Screen Frames)
- **Status**: ✅ Complete
- **Crate**: `crates/transport`
- **Key deliverables**: `ScreenReceiver::spawn()` — accepts streams, spawns per-stream drain tasks, drop-old-groups via `try_send`, `ReceiverHandle`

### P1.8 — Cursor Sharing via Datagrams
- **Status**: ✅ Complete
- **Crate**: `crates/transport`
- **Key deliverables**: `CursorSender::spawn()` (fires datagrams from mpsc channel), `CursorReceiver::spawn()` (exposes `watch::Receiver<Option<CursorUpdate>>`)
- **Tests**: cursor_update_serializes_under_mtu

### P1.9 — Basic Viewer UI
- **Status**: ✅ Complete
- **Crate**: `crates/app`
- **Key deliverables**: `ViewerApp` (egui), GPU texture upload via `ColorImage`, cursor overlay with click animation, FPS counter
- **Notes**: Behind `gui` feature flag. `watch::Receiver` for frame/cursor updates. Aspect-ratio-preserving scale.

### P1.10 — End-to-End Integration
- **Status**: ✅ Complete
- **Crate**: `crates/app`
- **Key deliverables**: `pairwire host` / `pairwire join <invite>` CLI (`cli.rs`), `run_host()`/`run_join()` async orchestrators (`session.rs`), graceful shutdown via `Ctrl-C`
- **Notes**: Encode/GUI paths are feature-gated. `watch` channels bridge async session to egui viewer thread.

## Notes

- iroh must be pinned carefully — v0.96 has a hole-punch regression (see LEARNINGS.md)
- FFmpeg encoder contexts are !Send — must stay on their dedicated thread
- scap capture callback must not be blocked — send to channel immediately
