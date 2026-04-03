# PLAN.md — Phased Build Plan (Agent-Ready)

> Each task is self-contained with clear inputs, outputs, acceptance criteria, and relevant docs.
> Agents: pick a task, read its dependencies, execute, validate with `just all`, commit with the task ID.

## Status Key

- `[ ]` — Not started
- `[~]` — In progress
- `[x]` — Completed
- `[!]` — Blocked (see notes)

---

## Phase 0: Project Foundation

> **Goal**: Green workspace that compiles, lints, tests, and gives agents a working baseline.

### P0.1: Initialize Cargo Workspace
- **Status**: `[x]`
- **Description**: Create root `Cargo.toml` workspace with all crate members. Create crate directories with minimal `lib.rs` / `main.rs` stubs that compile.
- **Crates to create**:
  - `crates/core` — shared types, config, error types
  - `crates/protocol` — wire protocol message definitions
  - `crates/transport` — iroh + moq-lite integration
  - `crates/capture` — screen capture + encoding
  - `crates/audio` — audio capture + Opus codec
  - `crates/input` — keyboard, mouse, clipboard
  - `crates/terminal` — PTY sharing
  - `crates/app` — binary entry point (UI)
- **Acceptance criteria**:
  - `just all` passes (check, lint, test, fmt)
  - Each crate has a `lib.rs` (or `main.rs` for app) with a doc comment describing its purpose
  - Workspace `Cargo.toml` uses resolver = "2"
  - Rust edition 2024 set in workspace
- **Docs**: `docs/05-concrete-architecture.md` (crate structure)

### P0.2: Core Error and Config Types
- **Status**: `[x]`
- **Depends on**: P0.1
- **Description**: Define the shared error enum, config structures, and common types in `crates/core`.
- **Deliverables**:
  - `Error` enum using `thiserror` covering: Transport, Capture, Encode, Decode, Audio, Protocol, Io
  - `Config` struct for session configuration (resolution, fps, codec, audio settings)
  - `PeerId` newtype wrapping iroh's EndpointId
  - `SessionId` type
  - Re-export common traits and types
- **Acceptance criteria**:
  - All types derive `Debug`, `Clone` where appropriate
  - `Error` implements `std::error::Error` via `thiserror`
  - Config types derive `serde::Serialize` + `serde::Deserialize`
  - Unit tests for serialization round-trip
  - `just all` passes

### P0.3: Protocol Message Types
- **Status**: `[x]`
- **Depends on**: P0.2
- **Description**: Define all wire protocol message types in `crates/protocol`. These are the types serialized with `postcard` and sent over the network.
- **Deliverables**:
  - `Handshake` — version, capabilities, display info, supported codecs
  - `ControlMessage` — session mgmt, permission changes, capability negotiation
  - `KeyEvent` — key code, modifiers, pressed/released, timestamp
  - `MouseClick` — position, button, pressed/released, timestamp
  - `MouseScroll` — position, delta, timestamp
  - `ClipboardSync` — format (text/image/files), data
  - `CursorUpdate` — peer_id, position, visible, click animation
  - `Heartbeat` — RTT, loss rate, bitrate
  - `ScreenFrameHeader` — timestamp, flags (keyframe, etc.)
  - Protocol version constant
- **Acceptance criteria**:
  - All types derive `Serialize`, `Deserialize`, `Debug`, `Clone`
  - All types have `#[cfg(test)]` round-trip tests with `postcard`
  - `CursorUpdate` serialized size < 50 bytes (verified by test)
  - `Heartbeat` serialized size < 50 bytes (verified by test)
  - `just all` passes
- **Docs**: `docs/03-quic-transport-primitives.md` (protocol design section), `docs/05-concrete-architecture.md` (protocol design)

### P0.4: CONVENTIONS.md
- **Status**: `[x]`
- **Depends on**: P0.1
- **Description**: Create `CONVENTIONS.md` as a living document for patterns and conventions discovered during development. Seed it with initial conventions from AGENTS.md and the research docs.
- **Deliverables**:
  - `CONVENTIONS.md` at repo root
  - Sections: Error Handling, Async & Threading, Channel Patterns, Serialization, Logging, Testing, Naming
  - Each section has 1-2 concrete examples from the codebase or docs
  - Instruction at the top: "Update this file when you establish a new pattern"
- **Acceptance criteria**:
  - File exists and is referenced in CLAUDE.md read order
  - Covers all sections listed above with concrete guidance

### P0.5: Test Harness & Stubs
- **Status**: `[x]`
- **Depends on**: P0.2, P0.3
- **Description**: Create test utilities and mock/stub infrastructure so agents can write integration tests without real network connections, screen capture hardware, or audio devices.
- **Deliverables**:
  - `crates/core/src/test_utils.rs` (behind `#[cfg(test)]` or a `test-support` feature flag):
    - `MockEndpoint` — fake iroh endpoint that connects two in-memory peers via channels
    - `FakeFrameSource` — generates synthetic BGRA frames (solid color or gradient with a frame counter) at configurable FPS
    - `FakeAudioSource` — generates silent or sine-wave PCM samples at configurable sample rate
    - `LoopbackChannel<T>` — bidirectional channel pair for testing send/recv patterns
  - `crates/protocol/src/test_utils.rs`:
    - Helper functions to create populated instances of every protocol message type (`make_handshake()`, `make_key_event()`, etc.)
    - `assert_round_trip<T: Serialize + DeserializeOwned>(value: &T)` — serialize then deserialize, assert equality
  - Test fixture data:
    - `tests/fixtures/` directory at workspace root
    - `tests/fixtures/README.md` explaining what goes here (encoded H.264 NAL samples, sample Opus frames, etc. — added later as needed)
- **Acceptance criteria**:
  - `MockEndpoint` can simulate a bidirectional connection: peer A sends bytes, peer B receives them
  - `FakeFrameSource` produces frames of correct size (width * height * 4 bytes for BGRA)
  - `FakeAudioSource` produces correct sample count for a given duration
  - `assert_round_trip` works for all protocol message types
  - All test helpers have doc comments explaining usage
  - `just all` passes

---

## Phase 1: Proof of Concept — Screen Sharing Over iroh

> **Goal**: Two peers share a screen and cursor over iroh P2P. No audio, no moq-lite yet. macOS only.
> **Estimated effort**: 4-6 weeks
> **Docs**: All of `docs/01` through `docs/05`

### P1.1: iroh Endpoint Setup
- **Status**: `[x]`
- **Depends on**: P0.2, P0.5
- **Description**: Create the transport layer in `crates/transport`. Set up iroh endpoint creation, invite link generation/parsing, and peer connection.
- **Deliverables**:
  - `TransportConfig` — relay URL, secret key path, ALPN protocol name
  - `Transport` struct wrapping `iroh::Endpoint` (singleton)
  - `InviteLink` — struct encoding endpoint_id + relay_url + session_token, with `Display` (to URL) and `FromStr` (from URL) impls
  - `connect(invite: &InviteLink) -> Result<Connection>` — connect to a peer
  - `accept() -> Result<Connection>` — accept incoming connection
  - `Connection` wrapper providing typed access to streams and datagrams
- **Acceptance criteria**:
  - Integration test: two iroh endpoints connect in-process using `MockEndpoint` or iroh's built-in test helpers
  - Invite link round-trips through `Display` → `FromStr`
  - `just all` passes
- **Docs**: `docs/01-iroh-noq-connectivity.md`

### P1.2: Raw Stream & Datagram Helpers
- **Status**: `[x]`
- **Depends on**: P1.1
- **Description**: Add typed stream and datagram helpers to the `Connection` wrapper. These are the building blocks for all data exchange.
- **Deliverables**:
  - `open_control_stream() -> Result<(ControlSend, ControlRecv)>` — bidirectional, priority 0
  - `open_input_stream() -> Result<InputSend>` — unidirectional, priority 1
  - `send_datagram(msg: &CursorUpdate) -> Result<()>` — serialize + send
  - `recv_datagram() -> Result<CursorUpdate>` — recv + deserialize
  - `send_message<T: Serialize>(stream, msg: &T)` — length-prefixed postcard write
  - `recv_message<T: DeserializeOwned>(stream) -> Result<T>` — length-prefixed postcard read
- **Acceptance criteria**:
  - Test: send and receive each message type over mock connection
  - Test: datagram payloads stay under 1200 bytes
  - `just all` passes
- **Docs**: `docs/03-quic-transport-primitives.md`

### P1.3: Screen Capture Thread
- **Status**: `[x]`
- **Depends on**: P0.2
- **Description**: Implement the capture thread in `crates/capture`. Uses scap to capture the primary display and sends raw frames to a channel.
- **Deliverables**:
  - `CaptureConfig` — fps, resolution, output format, target display/window
  - `CaptureThread` — owns scap `Capturer`, runs on dedicated OS thread
  - `start() -> Receiver<RawFrame>` — starts capture, returns channel receiver
  - `stop()` — gracefully stops capture
  - `RawFrame` — struct with pixel data (`bytes::Bytes`), width, height, format, timestamp
- **Acceptance criteria**:
  - Integration test using `FakeFrameSource` verifying frames arrive on channel at expected rate
  - Real capture test (gated behind `#[ignore]` or feature flag) that captures 10 frames from primary display
  - `just all` passes
- **Docs**: `docs/04-screen-capture-encode-pipeline.md`

### P1.4: FFmpeg H.264 Encode Thread
- **Status**: `[x]`
- **Depends on**: P1.3
- **Description**: Implement the encode thread. Takes raw frames from capture channel, encodes to H.264 NAL units via FFmpeg hardware encoder, sends to output channel.
- **Deliverables**:
  - `EncodeConfig` — codec name, resolution, bitrate, keyframe interval, low-latency tuning params
  - `EncodeThread` — owns FFmpeg encoder context, dedicated OS thread
  - `start(input: Receiver<RawFrame>) -> Receiver<EncodedPacket>` — starts encode loop
  - `EncodedPacket` — NAL unit data, timestamp, keyframe flag, size
  - Hardware encoder selection: try `h264_videotoolbox` (macOS), fall back to `libx264`
  - Static frame detection: skip encoding when frame is identical to previous (byte comparison or quick hash)
- **Acceptance criteria**:
  - Test with `FakeFrameSource`: synthetic frames encode successfully
  - Encoded packets contain valid H.264 NAL units (check start codes)
  - Keyframes appear at configured interval
  - Static frame skipping works (feed identical frames, verify fewer packets output)
  - `just all` passes
- **Docs**: `docs/04-screen-capture-encode-pipeline.md`

### P1.5: FFmpeg H.264 Decode Thread
- **Status**: `[x]`
- **Depends on**: P1.4
- **Description**: Implement the decode thread. Takes encoded packets, decodes to raw frames for rendering.
- **Deliverables**:
  - `DecodeThread` — owns FFmpeg decoder context, dedicated OS thread
  - `start(input: Receiver<EncodedPacket>) -> Receiver<DecodedFrame>`
  - `DecodedFrame` — pixel data, width, height, format, timestamp
  - Hardware decoder selection: try `h264_videotoolbox` (macOS), fall back to software
- **Acceptance criteria**:
  - Round-trip test: `FakeFrameSource` → encode → decode → verify dimensions and frame count match
  - `just all` passes
- **Docs**: `docs/04-screen-capture-encode-pipeline.md`

### P1.6: Network Sender — Screen Frames Over QUIC
- **Status**: `[x]`
- **Depends on**: P1.2, P1.4
- **Description**: Send encoded screen frames over a reliable QUIC stream to the remote peer. Each keyframe starts a new stream (mimicking moq-lite's group model for easy migration later).
- **Deliverables**:
  - `ScreenSender` — reads from encoded packet channel, writes to QUIC streams
  - Frame header format: timestamp (u48) + flags (u8) + reserved (u8) + payload
  - New unidirectional stream per keyframe group
  - Priority set on streams (priority 3)
- **Acceptance criteria**:
  - Integration test with mock connection: send 30 frames, receive all 30 with correct headers
  - Keyframe groups create new streams
  - `just all` passes
- **Docs**: `docs/03-quic-transport-primitives.md`, `docs/05-concrete-architecture.md`

### P1.7: Network Receiver — Screen Frames
- **Status**: `[x]`
- **Depends on**: P1.6, P1.5
- **Description**: Receive screen frames from remote peer and feed to decoder.
- **Deliverables**:
  - `ScreenReceiver` — accepts unidirectional streams, parses frame headers, sends to decode channel
  - Handle stream multiplexing (multiple groups in flight)
  - Drop old groups if decoder falls behind
- **Acceptance criteria**:
  - End-to-end test with mock: send frames → network → receive → verify ordering and content
  - `just all` passes

### P1.8: Cursor Sharing via Datagrams
- **Status**: `[~]` (transport layer done; not wired end-to-end — see notes)
- **Depends on**: P1.2
- **Description**: Send and receive cursor position updates via QUIC datagrams.
- **Deliverables**:
  - `CursorSender` — polls cursor position, sends `CursorUpdate` datagrams at 60Hz when moving, throttles when still
  - `CursorReceiver` — receives datagrams, exposes latest cursor position via `Arc<AtomicCell<CursorPosition>>`
  - Platform cursor position reading (macOS: `CGEvent`)
- **Notes**:
  - `CursorSender`/`CursorReceiver` transport code exists and works.
  - **NOT wired correctly in session.** Current flow is backwards: guest has `CursorSender` and host has `CursorReceiver`, but nothing feeds cursor positions into the guest's sender. The correct flow for screen sharing is: **host captures its own cursor → sends to guest → guest viewer renders overlay**.
  - The host needs a `CursorSender` that polls local cursor position via `CGEvent` and sends datagrams.
  - The guest needs a `CursorReceiver` whose output feeds into the viewer's `cursor_tx` watch channel.
  - Viewer overlay code already exists (`viewer.rs` lines 106-137) and renders a crosshair when data arrives.
- **Remaining work**:
  - Implement platform cursor position polling (macOS: `CGEvent::new(CGEventType::Null)` → `.location()`)
  - Wire host-side `CursorSender` with a polling loop
  - Wire guest-side `CursorReceiver` output to the viewer's `cursor_tx`
- **Acceptance criteria**:
  - Test: send 100 cursor updates, verify latest position is accessible (not all — datagrams are lossy)
  - Verify `CursorUpdate` serialized size < 50 bytes
  - `just all` passes
- **Docs**: `docs/03-quic-transport-primitives.md` (datagram section)

### P1.9: Basic Viewer UI
- **Status**: `[x]`
- **Depends on**: P1.5, P1.8
- **Description**: Minimal UI window that displays the decoded remote screen and overlays the remote cursor.
- **Deliverables**:
  - egui (or iced) window that displays a texture from `DecodedFrame`
  - GPU texture upload (wgpu) — update texture each frame from the decode channel
  - Remote cursor overlay (colored dot/arrow at cursor position)
  - FPS counter and connection status display
  - Window title shows peer EndpointId (truncated)
- **Notes**: Fully implemented in `crates/app/src/viewer.rs`. egui/wgpu viewer with BGRA→RGBA conversion, scaled texture display, cursor crosshair overlay, FPS counter, and reconnection status overlay. Cursor overlay renders correctly when data is provided (blocked on P1.8 wiring).
- **Acceptance criteria**:
  - Window opens, displays frames from `FakeFrameSource` (no network needed)
  - Cursor overlay renders at correct position
  - FPS counter works
  - `just all` passes

### P1.10: End-to-End Integration — Two Peers
- **Status**: `[x]` (screen + audio working; cursor deferred to P3.5.1)
- **Depends on**: P1.1 through P1.9
- **Description**: Wire everything together. Two instances of the app connect via iroh, one shares screen + cursor, the other views.
- **Deliverables**:
  - CLI interface: `pairwire host` (captures and shares) / `pairwire join <invite>` (views)
  - Host: capture → encode → send + cursor send
  - Guest: receive → decode → display + cursor receive
  - Invite link generation and display on host startup
  - Graceful shutdown (Ctrl+C)
- **Notes**: Screen + audio streaming verified end-to-end. Cursor overlay infrastructure exists in viewer but awaiting P3.5.1 for data flow wiring. Stream multiplexing conflict fixed via `UniStreamRouter` (commit 0dc73fa). Retina resolution mismatch fixed via lazy scaler init.
- **Acceptance criteria**:
  - ~~Cursor position updates visible~~ (deferred to P3.5.1)
  - Manual test: run host on one terminal, join on another (same machine is fine — iroh handles loopback)
  - Screen appears in viewer within 3 seconds
  - Clean shutdown without panics
  - `just all` passes
- **Performance targets** (measure and record in LEARNINGS.md):
  - Screen latency < 100ms
  - Cursor latency < 50ms (once P3.5.1 complete)
  - CPU usage < 15% at 1080p 30fps

---

## Phase 2: Protocol Layer + Remote Control

> **Goal**: Replace raw streams with moq-lite, add keyboard/mouse input, clipboard sync, quality adaptation.
> **Estimated effort**: 3-4 weeks
> **Docs**: `docs/02-moq-lite-vs-moq-transport.md`, `docs/05-concrete-architecture.md`

### P2.1: moq-lite Integration — Screen Track
- **Status**: `[x]`
- **Depends on**: P1.10
- **Description**: Replace the raw QUIC stream screen transport with moq-lite tracks. Each keyframe group becomes a moq-lite group.
- **Deliverables**:
  - moq-lite session setup over iroh connection
  - Screen published as a moq-lite track (priority 3)
  - Groups map to keyframe intervals
  - Subscriber receives groups and feeds to decoder
- **Acceptance criteria**:
  - Same end-to-end behavior as P1.10 but over moq-lite
  - Priority is correctly set on the track
  - `just all` passes
- **Docs**: `docs/02-moq-lite-vs-moq-transport.md`

### P2.1.1: Adopt `hang` — Media Framing Layer
- **Status**: `[x]`
- **Depends on**: P2.1, P3.1
- **Description**: Replace the custom `ScreenFrameHeader` (manual 8-byte header) and manual audio framing with `hang` from moq-dev/moq. `hang` is the media layer that sits between moq-lite and the application — it provides a catalog track (advertising codec/resolution/bitrate metadata), WebCodecs-compatible frame types, and typed track naming. The original architecture doc (docs/05-concrete-architecture.md) listed `hang` as a required layer but it was skipped during initial integration. This task adds it.
- **Why now**: Required before Phase W (web viewer). hang's frame format is WebCodecs-native — PW.3 can feed frames directly to the browser's `VideoDecoder` and `AudioDecoder` APIs without custom header parsing. Doing this before the web viewer saves a full refactor later.
- **Deliverables**:
  - Add `hang` dependency from moq-dev/moq workspace
  - Replace `ScreenFrameHeader` (custom 8-byte timestamp+flags) with `hang::Frame` (timestamp + codec payload)
  - Publish screen via `hang::BroadcastProducer` with a catalog track advertising codec, resolution, keyframe interval
  - Subscribe via `hang::BroadcastConsumer` — read catalog, then consume frames
  - Extend to audio tracks: `hang` track named `audio/host`, `audio/guest_N` (per-source, supports multi-party fan-out)
  - Codec negotiation in handshake can be simplified: hang catalog replaces the manual codec field in `Handshake`
- **Multi-party note**: Per-source audio tracks (`audio/host`, `audio/guest_1`, etc.) map directly to hang's broadcast model — each peer publishes their own named track. Multi-party audio fan-out (P5.1) becomes "subscribe to all audio/* tracks except your own" with no extra protocol design.
- **Simulcast / on-demand note**: hang supports multiple quality tracks in one broadcast catalog (`screen/1080p`, `screen/720p`, `audio/hifi`, `audio/asr`). This is the foundation for on-demand simulcast and the Whisper ASR audio track idea.
- **Acceptance criteria**:
  - Screen and audio work end-to-end using hang framing (no regression from P2.1/P3.1)
  - Catalog track is published and readable by subscribers
  - `ScreenFrameHeader` type removed from codebase
  - `just all` passes
- **Docs**: `docs/02-moq-lite-vs-moq-transport.md` (hang layer section)

### P2.2: Session Handshake & Capability Negotiation
- **Status**: `[x]`
- **Depends on**: P1.2, P0.3
- **Description**: Implement the control stream handshake. Peers exchange capabilities, display info, and supported codecs before starting media.
- **Deliverables**:
  - Handshake exchange on the control bidirectional stream
  - Capability negotiation (screen, audio, remote_control, annotations)
  - Codec negotiation (intersection of supported codecs)
  - Display info exchange (resolution, scale factor)
  - Version check — reject incompatible protocol versions
- **Acceptance criteria**:
  - Test: two peers negotiate successfully with matching capabilities
  - Test: version mismatch produces a clean error
  - Test: codec negotiation picks the best mutual codec
  - `just all` passes

### P2.3: Keyboard & Mouse Input Forwarding
- **Status**: `[x]` (code complete — session wiring is P3.5.2)
- **Depends on**: P2.2
- **Description**: Implement remote control in `crates/input`. Guest sends keyboard and mouse events; host injects them into the OS.
- **Deliverables**:
  - `InputSender` — captures local keyboard/mouse events, serializes, sends on input stream
  - `InputReceiver` — receives events, injects into OS via platform APIs
  - macOS injection: `CGEvent` posting
  - Permission model: host must explicitly grant control
  - `ControlHandoff` message for granting/revoking permissions
- **Notes**:
  - `InputSender` and `InputReceiver` fully implemented in `crates/input/src/sender.rs` and `receiver.rs`.
  - Protocol types (`KeyEvent`, `MouseClick`, `MouseScroll`) defined in `crates/protocol/src/input.rs`.
  - **NOT instantiated in `crates/app/src/session.rs`** — no input stream is opened or accepted.
- **Remaining work**: Wire `InputSender` in `run_join()` and `InputReceiver` in `run_host()`. Open/accept input stream. Add permission grant UI/flow.
- **Acceptance criteria**:
  - Test: serialize and deserialize all input event types
  - Manual test: guest types, host sees keystrokes
  - Permission check: events are rejected when control not granted
  - `just all` passes
- **Docs**: `docs/05-concrete-architecture.md` (remote control protocol section)

### P2.4: Clipboard Sync
- **Status**: `[x]` (code complete — session wiring is P3.5.5)
- **Depends on**: P2.2
- **Description**: Bidirectional clipboard synchronization between peers.
- **Deliverables**:
  - Clipboard monitoring (detect changes)
  - `ClipboardSync` message with format detection (text, image, files)
  - Configurable: auto-sync or manual
  - Size limit to prevent accidental large transfers
- **Notes**:
  - `ClipboardSender`/`ClipboardReceiver` fully implemented in `crates/input/src/clipboard.rs` with polling, auto-sync flag, 1MB size limit.
  - **NOT spawned in session** — no clipboard monitors running.
- **Remaining work**: Wire into `run_host()` and `run_join()`.
- **Acceptance criteria**:
  - Test: text clipboard round-trips between peers
  - Large clipboard data (>1MB) is rejected or chunked
  - `just all` passes

### P2.5: Quality Adaptation
- **Status**: `[x]` (code complete — session wiring is P3.5.6)
- **Depends on**: P2.1
- **Description**: Monitor QUIC connection stats and adapt screen quality dynamically.
- **Deliverables**:
  - Monitor RTT and loss via QUIC/iroh stats
  - Adaptation rules:
    - Loss > 5%: reduce resolution (1080p → 720p)
    - Loss > 15%: reduce FPS (30 → 15)
    - RTT > 200ms: reduce FPS, increase keyframe interval
  - Recovery when metrics improve
  - `Heartbeat` datagram exchange for diagnostic visibility
- **Notes**:
  - `QualityMonitor` fully implemented in `crates/transport/src/quality.rs` with RTT/loss polling, tier adaptation (Full/Reduced/Degraded), and recovery hysteresis.
  - **NOT instantiated in session** — encode thread never receives adapted configs via watch channel.
- **Remaining work**: Spawn `QualityMonitor` in `run_host()`, connect its output watch channel to the encode thread to dynamically adjust bitrate/fps/resolution.
- **Acceptance criteria**:
  - Test with simulated degradation: verify encoder config changes
  - Heartbeat messages round-trip correctly
  - `just all` passes
- **Docs**: `docs/05-concrete-architecture.md` (degradation handling)

### P2.6: Skip Static Frames Optimization
- **Status**: `[x]`
- **Depends on**: P1.4
- **Description**: Detect when the screen hasn't changed and skip encoding entirely.
- **Deliverables**:
  - Frame comparison (block-level hash or byte comparison)
  - When static: don't submit to encoder, don't send over network
  - After N static frames, send a single "still alive" message
  - Resume encoding immediately when change detected
- **Acceptance criteria**:
  - Test: feed 100 identical frames, verify < 5 encoded packets
  - Test: feed alternating frames, verify all are encoded
  - Bandwidth measurement: static screen = near-zero network usage
  - `just all` passes

---

## Phase 3: Audio + Multi-Platform + Polish

> **Goal**: Full pair-programming experience with audio. Windows + Linux support.
> **Estimated effort**: 4-6 weeks
> **Docs**: `docs/04-screen-capture-encode-pipeline.md` (audio section), `docs/05-concrete-architecture.md`

### P3.1: Audio Capture + Opus Encoding
- **Status**: `[x]`
- **Depends on**: P2.1
- **Description**: Capture microphone audio via cpal, encode with Opus, send as moq-lite audio track.
- **Deliverables**:
  - Audio capture thread using cpal (48kHz, mono)
  - Opus encoding (20ms frames, 32kbps)
  - Published as moq-lite track (priority 0 — highest)
  - Voice activity detection — don't send silence
- **Notes**: Currently uses raw moq-lite track. P2.1.1 (hang adoption) will rename this to `audio/host` and add catalog metadata. Track naming convention `audio/<peer_id>` is required for multi-party fan-out in P5.1.
- **Acceptance criteria**:
  - Test with `FakeAudioSource`: Opus encode/decode round-trip produces same sample count
  - Manual test: speak into mic, hear output on peer
  - `just all` passes

### P3.2: Audio Decode + Playback
- **Status**: `[x]`
- **Depends on**: P3.1
- **Description**: Receive remote audio track, decode Opus, play through speakers/headphones.
- **Deliverables**:
  - Opus decoder
  - cpal output stream
  - Jitter buffer (simple: 40-60ms buffer to smooth delivery)
  - Volume control
- **Acceptance criteria**:
  - End-to-end audio: capture → encode → network → decode → playback
  - No audible glitches under normal network conditions
  - `just all` passes

### P3.2.1: Bidirectional Audio (Guest Mic → Host)
- **Status**: `[ ]`
- **Depends on**: P3.1, P3.2
- **Description**: P3.1 and P3.2 implement one-directional audio (host mic → guest speakers). For real pair programming both peers need to speak and hear each other. The guest/joiner must also capture their microphone and publish it as a moq-lite audio track; the host receives and plays it.
- **Deliverables**:
  - Wire guest-side audio capture + Opus encode in `run_join()` (mirrors the host-side pipeline in `run_host()`)
  - Wire host-side audio decode + playback of the guest's track in `run_host()`
  - Both peers publish and subscribe to each other's audio track (two separate moq-lite tracks: one per direction)
  - Ensure volume/mute controls apply to the correct track on each side
- **Notes**: Current state — host mic → guest speakers works (verified in first real-device test, 2026-04-01). Guest mic → host is entirely absent from the session wiring.
- **Acceptance criteria**:
  - Guest speaks, host hears them
  - Both directions work simultaneously without echo (echo cancellation is stretch — AEC is a separate concern)
  - `just all` passes

### P3.3: Windows Support
- **Status**: `[x]` (CI config added; runtime testing deferred to hardware)
- **Depends on**: P1.10
- **Description**: Ensure all crates compile and work on Windows.
- **Key work**:
  - scap: verify Desktop Duplication backend works
  - FFmpeg: NVENC / MediaFoundation encoder selection
  - Input injection: Windows `SendInput` API
  - Build and test in CI
- **Acceptance criteria**:
  - `just all` passes on Windows
  - Manual test: screen sharing works between Windows peers

### P3.4: Linux Support
- **Status**: `[x]` (CI config added; runtime testing deferred to hardware)
- **Depends on**: P1.10
- **Description**: Ensure all crates compile and work on Linux (Wayland + X11).
- **Key work**:
  - scap: verify PipeWire backend works
  - FFmpeg: VA-API / NVENC encoder selection
  - Input injection: Wayland input simulation (more restricted than X11)
  - Build and test in CI
- **Acceptance criteria**:
  - `just all` passes on Linux
  - Manual test: screen sharing works on Wayland

### P3.5: Session Persistence & Reconnection
- **Status**: `[x]`
- **Depends on**: P2.2
- **Description**: Handle network disruptions gracefully. Automatic reconnection with session state preservation.
- **Deliverables**:
  - Detect connection loss
  - Automatic reconnection via iroh (0-RTT when possible)
  - Resume session without re-handshake if session token is still valid
  - "Reconnecting..." UI overlay
  - Timeout after N seconds → show "Connection lost" with retry button
- **Acceptance criteria**:
  - Test: simulate connection drop, verify reconnection within 2 seconds
  - UI shows appropriate status during reconnection
  - `just all` passes

### P3.6: System Tray / Menu Bar Integration
- **Status**: `[x]`
- **Notes**: tray-icon + muda; Copy Invite Link, Disconnect, Quit; invite URL surfaced via oneshot channel

### P3.7: CI Workflow (macOS + Linux + Windows)
- **Status**: `[x]`
- **Notes**: `.github/workflows/ci.yml`; macOS runs encode feature with FFmpeg; Linux/Windows default features

### P3.8: Clippy Pedantic Pass
- **Status**: `[x]`
- **Notes**: Zero warnings under `-W clippy::pedantic`; 144 → 0 warnings across all 8 crates

---

## Phase 3.5: Interaction Layer (Tuple-Style Collaboration)

> **Goal**: Build the full interaction model — bidirectional cursors, control takeover, drawing, reactions. This is what turns "screen viewer" into "pair programming tool."
>
> **Design philosophy**: Both peers are equal participants. Either can point, draw, type, react. The screen is a shared canvas with two cursors, not a one-way broadcast.
>
> **Estimated effort**: 2-3 weeks
> **Priority**: Do this before Phase 4. It's the core UX.

### Interaction Model

```
┌─────────────────────────────────────────────────────┐
│                  SHARED SCREEN                       │
│                                                      │
│   🔴 Host cursor (baked into capture by scap)        │
│   🔵 Guest cursor (overlay, sent via datagrams)      │
│                                                      │
│   ✏️  Drawing strokes (either peer, distinct colors)  │
│   😀 Reactions (float up from cursor, fade out)      │
│                                                      │
│   [Active controller]: whoever clicked last          │
│   [Observer]: sees everything, can draw & react      │
└─────────────────────────────────────────────────────┘
```

**Cursor rules:**
- Host cursor is always visible in the screen capture (scap `show_cursor: true`). No overlay needed — it's "free."
- Guest cursor is an overlay rendered on both sides: on the host's screen (drawn by host app) and in the guest's viewer (drawn locally for zero-latency feedback).
- Both cursors always visible. No hiding, no "your turn" gating.

**Control rules:**
- Default: host has control (their real OS cursor).
- Guest clicks in viewer → `ControlRequest` sent → host auto-accepts (configurable) → guest's keyboard/mouse events injected on host.
- Host moves their real mouse → automatically reclaims control. Guest input paused until they click again.
- Either side can draw or react at any time regardless of who has control.

**Wire protocol additions** (new types in `crates/protocol/`):
- `CursorUpdate` — already exists. Add `peer_role: PeerRole` field.
- `ControlRequest { peer_id }` / `ControlGrant { peer_id }` — on control stream.
- `DrawStroke { peer_id, color, points, timestamp }` — on reliable stream.
- `DrawPoint { peer_id, x, y }` — via datagram for real-time preview.
- `Reaction { peer_id, emoji, x, y, timestamp }` — via datagram.

### P3.5.1: Bidirectional Cursor Presence
- **Status**: `[ ]`
- **Depends on**: P1.8
- **Description**: Both peers see each other's cursor at all times.
- **Code locations**:
  - `crates/transport/src/cursor.rs` — `CursorSender` / `CursorReceiver` (transport layer, already works)
  - `crates/app/src/session.rs::run_host()` ~line 115 — currently spawns `CursorReceiver` only. **Add**: `CursorSender` with a cursor-polling loop.
  - `crates/app/src/session.rs::run_join()` ~line 192 — `cursor_update_tx` created but **never fed data**. Wire viewer mouse position → `cursor_update_tx`. Forward `CursorReceiver` output → `_cursor_tx` watch channel (line 152).
  - `crates/app/src/viewer.rs` ~line 105 — cursor overlay rendering code. **Already works** when data arrives via `cursor_rx`.
  - **New**: Platform cursor polling function (macOS: `CGEvent::new(CGEventType::Null).location()`) in `crates/input/` or `crates/capture/`.
- **Deliverables**:
  - **Host side**: Poll local cursor position (macOS: `CGEvent`), send `CursorUpdate` datagrams to guest at up to 60Hz. Receive guest's `CursorUpdate` and render a colored overlay on the host display (or in the host's tray/status UI if no host viewer).
  - **Guest side**: Track mouse position within the viewer window, map to host display coordinates, send `CursorUpdate` datagrams to host. Receive host's `CursorUpdate` — but since the host cursor is already in the captured video, this is optional (useful for coordinate-aware features like "snap to host cursor").
  - Distinct cursor colors: host = red crosshair, guest = blue crosshair (configurable later).
  - Throttle: only send when position changes by >2px. Idle after 500ms of no movement.
- **Acceptance criteria**:
  - Guest viewer shows host cursor movements in real-time (via screen capture, already works)
  - Host sees guest cursor as a colored overlay
  - Cursor updates are <50 bytes per datagram
  - Manual test: move mouse on guest viewer, see crosshair move on host
  - `just all` passes

### P3.5.2: Control Takeover
- **Status**: `[~]` (session wiring done; viewer capture + coordinate mapping remaining)
- **Depends on**: P3.5.1, P2.3
- **Description**: Guest can take keyboard/mouse control by clicking in the viewer. Host reclaims by moving their real mouse. Like Tuple's "click to drive" model.
- **Code locations**:
  - `crates/input/src/sender.rs` — `InputSender::new(stream).spawn(input_rx)` (code complete, needs stream)
  - `crates/input/src/receiver.rs` — `InputReceiver::new(stream).spawn()` (code complete, needs stream)
  - `crates/app/src/session.rs::run_host()` — **Add after line ~115**: accept input stream via `conn.accept_uni()`, spawn `InputReceiver`
  - `crates/app/src/session.rs::run_join()` — **Add after line ~195**: open input stream via `conn.open_input_stream()`, spawn `InputSender`
  - `crates/app/src/viewer.rs` — **Add**: capture egui keyboard/mouse events, send via `input_tx` channel to `InputSender`
  - `crates/protocol/src/control.rs` — **Add**: `ControlRequest`/`ControlGrant`/`ControlRevoke` variants to `ControlMessage`
- **Deliverables**:
  - `ControlState` enum: `HostActive | GuestActive | GuestRequested`
  - `ControlRequest` / `ControlGrant` messages on the control stream
  - Guest click in viewer → sends `ControlRequest` → host auto-grants (default) or prompts
  - While guest is active: viewer captures keyboard events + mouse clicks/moves → `InputSender` forwards to host → `InputReceiver` injects via `CGEvent`
  - Host moves their real mouse → `ControlRevoke` sent → guest returns to observer mode
  - Visual indicator in viewer: "You have control" / "Host has control" badge
  - Coordinate mapping: guest viewer pixels → host display pixels (accounting for scale factor and viewer window size)
- **Acceptance criteria**:
  - Manual test: guest clicks in viewer, types — keystrokes appear on host
  - Manual test: host wiggles mouse — control returns to host, guest typing stops injecting
  - Coordinate mapping is accurate (click targets correct UI elements on host)
  - `just all` passes

### P3.5.3: Drawing & Annotations
- **Status**: `[ ]`
- **Depends on**: P3.5.1
- **Description**: Either peer can draw on the shared screen. Strokes are overlays (not baked into capture). Inspired by Tuple's laser pointer + persistent drawing.
- **Deliverables**:
  - **Two drawing modes**:
    - **Laser pointer** (default): hold a hotkey (e.g., Cmd/Ctrl) and move mouse. Trail fades after ~2 seconds. For "look here" moments.
    - **Persistent draw**: toggle with a keybind. Strokes remain until cleared.
  - Drawing data flow:
    - Real-time: `DrawPoint` datagrams for low-latency stroke preview (lossy is fine — worst case a point is skipped)
    - Completion: `DrawStroke` on reliable stream with full point list for accurate rendering
  - Each peer gets a distinct color (derived from peer_id hash or configurable)
  - Clear all: hotkey or control message to erase the canvas
  - Render: egui painter overlay on top of the screen texture, below the cursor overlay
- **Code locations**:
  - `crates/protocol/src/drawing.rs` — **New**: `DrawPoint`, `DrawStroke`, `DrawClear`, `DrawMode` types
  - `crates/protocol/src/lib.rs` — **Add**: `pub mod drawing;` + re-exports
  - `crates/protocol/src/control.rs` — **Add**: `DrawClear` variant to `ControlMessage` enum (line 70)
  - `crates/transport/src/drawing.rs` — **New**: `DrawingSender` (datagrams for `DrawPoint`, stream for `DrawStroke`) + `DrawingReceiver` (mirrors cursor.rs pattern at line 77)
  - `crates/transport/src/connection.rs` — **Add**: `send_draw_datagram()` / `recv_draw_datagram()` methods (pattern: `send_cursor_datagram` at existing datagram methods)
  - `crates/app/src/viewer.rs:76` — **Modify**: `ViewerApp` gets `drawing_rx: watch::Receiver<Vec<DrawStroke>>`, render strokes in `update()` between screen texture (line 98) and cursor overlay (line 106)
  - `crates/app/src/session.rs` — **Add**: spawn `DrawingSender` + `DrawingReceiver` in both `run_host()` and `run_join()`
- **Protocol types**:
  - `DrawPoint { peer_id: u8, x: u16, y: u16 }` — datagram, <20 bytes
  - `DrawStroke { peer_id: u8, color: [u8; 3], points: Vec<(u16, u16)>, mode: DrawMode }` — stream
  - `DrawClear { peer_id: u8 }` — clear one peer's strokes or all
- **Acceptance criteria**:
  - Both peers can draw simultaneously with distinct colors
  - Laser pointer trail fades smoothly over ~2 seconds
  - Persistent strokes survive until explicitly cleared
  - Drawing works regardless of who has control
  - `just all` passes

### P3.5.4: Reactions
- **Status**: `[ ]`
- **Depends on**: P3.5.1
- **Description**: Quick emoji reactions that appear at the sender's cursor position and float upward. For "nice!", "I see the bug", "wait what?" moments without interrupting voice chat.
- **Deliverables**:
  - Predefined reaction set: 👍 👎 ❤️ 🎉 ❓ 🐛 💡 🔥 (8 reactions, keybind or picker)
  - `Reaction` datagram: `{ peer_id, emoji_id: u8, x: u16, y: u16 }` — <15 bytes
  - Render: emoji appears at cursor position, floats up ~50px over 2 seconds, fades out
  - Multiple reactions can be in-flight simultaneously (max 10 visible, drop oldest)
  - Hotkey: number keys 1-8 while holding a modifier, or a floating picker in the viewer
- **Code locations**:
  - `crates/protocol/src/reaction.rs` — **New**: `Reaction { peer_id: u8, emoji_id: u8, x: u16, y: u16 }` type + `EMOJI_SET` const array
  - `crates/protocol/src/lib.rs` — **Add**: `pub mod reaction;` + re-exports
  - `crates/transport/src/reaction.rs` — **New**: `ReactionSender` / `ReactionReceiver` (datagram-only, mirrors cursor.rs pattern at line 23/77)
  - `crates/transport/src/connection.rs` — **Add**: `send_reaction_datagram()` / `recv_reaction_datagram()` methods (pattern: `send_cursor_datagram`)
  - `crates/app/src/viewer.rs:17` — **Modify**: `ViewerApp` gets `reaction_rx: watch::Receiver<Vec<ActiveReaction>>` where `ActiveReaction` tracks position + spawn time for fade animation; render in `update()` after cursor overlay (line 138)
  - `crates/app/src/session.rs` — **Add**: spawn `ReactionSender` + `ReactionReceiver` in both `run_host()` and `run_join()`
- **Acceptance criteria**:
  - Send a reaction, see it appear on both screens at the sender's cursor position
  - Reactions fade out after ~2 seconds
  - Works regardless of who has control
  - `just all` passes

### P3.5.5: Wire Clipboard Sync
- **Status**: `[ ]`
- **Depends on**: P2.4
- **Description**: Wire `ClipboardSender`/`ClipboardReceiver` into session.
- **Code locations**:
  - `crates/input/src/clipboard.rs` — `ClipboardSender::new(event_tx, auto_sync).spawn()` (line 71), `ClipboardReceiver` (line 171) — code complete
  - `crates/app/src/session.rs::run_host()` — **Add**: spawn `ClipboardSender` + `ClipboardReceiver` after handshake
  - `crates/app/src/session.rs::run_join()` — **Add**: spawn `ClipboardSender` + `ClipboardReceiver` after handshake
  - Both share clipboard events via the existing input stream (clipboard uses `InputMessage::Clipboard` variant)
- **Deliverables**:
  - Both host and guest spawn clipboard monitors
  - Copy on one side appears on the other within ~500ms
  - Visual toast: "Clipboard synced" indicator
- **Acceptance criteria**:
  - Manual test: copy text on host, paste on guest — text matches
  - Large data (>1MB) is rejected with a warning
  - `just all` passes

### P3.5.6: Wire Quality Adaptation
- **Status**: `[ ]`
- **Depends on**: P2.5
- **Description**: Wire `QualityMonitor` into host session and connect to encode thread.
- **Code locations**:
  - `crates/transport/src/quality.rs` — `QualityMonitor::new(conn, base_config).spawn()` → returns `(handle, watch::Receiver<CaptureConfig>)` (line 114) — code complete
  - `crates/app/src/session.rs::run_host()` — **Add after line ~112**: spawn `QualityMonitor`, pass the `watch::Receiver<CaptureConfig>` to the encode thread so it can dynamically adjust bitrate/fps/resolution
  - `crates/capture/src/encode_thread.rs` — **Modify**: accept a `watch::Receiver<CaptureConfig>` and check for changes each frame, reinitializing encoder params when the config changes
- **Deliverables**:
  - `QualityMonitor` spawned in `run_host()`
  - Output watch channel connected to encode thread for dynamic bitrate/fps/resolution changes
  - Visual indicator in viewer: connection quality badge (green/yellow/red)
- **Acceptance criteria**:
  - Manual test: simulate high latency, verify encoder downgrades quality tier
  - When conditions improve, quality recovers
  - `just all` passes

---

## Phase 3.9: Hardening & Review Fixes

> **Goal**: Address deferred code review findings that need design or cross-cutting changes.
> **Ref**: `REVIEW.md` for full details on each finding.

### P3.9.1: Peer Authentication (R-C4)
- **Status**: `[x]`
- **Description**: Verify that the connecting peer's endpoint ID matches the one encoded in the invite link. Currently any peer can connect to any session.
- **Deliverables**:
  - `Transport::accept()` returns the peer ID for verification
  - Handshake verifies `conn.remote_id()` matches expected endpoint from invite
  - Reject mismatches with a clear error
- **Notes**: Implemented via session_token field in Handshake. Both peers send the token from the invite link; mismatch → PeerAuthFailed error.

### P3.9.2: Structured Error Architecture (R-M2, R-A5)
- **Status**: `[x]`
- **Description**: Replace string-wrapped error variants (`Transport(String)`, `Encode(String)`, etc.) with structured sub-enums. Enables retry logic, metrics, and user-facing diagnostics.
- **Deliverables**:
  - `TransportError`, `CodecError`, `ProtocolError` sub-enums with `From` impls
  - `#[from]` for `postcard::Error`, `std::io::Error`, `moq_lite::Error`
- **Notes**: Added Postcard(#[from]), PeerAuthFailed, InvalidConfig variants. String variants kept as fallback for less common errors.

### P3.9.3: Config Validation (R-M1)
- **Status**: `[x]`
- **Description**: `CaptureConfig` and `AudioConfig` allow invalid states (fps=0, width=0, channels=0). Add `validate()` or `TryFrom` with rejection of invalid values.
- **Deliverables**:
  - Reject `fps == 0`, `width == 0`, `height == 0`, `channels not in [1,2]`
  - Call validation at session startup
- **Notes**: validate() methods on CaptureConfig, AudioConfig, SessionConfig. 17 unit tests.

### P3.9.4: Audio Silence Markers (R-M6)
- **Status**: `[x]`
- **Description**: Skipped silence frames increment the timestamp but send nothing, causing decoder/jitter buffer discontinuities. Send an explicit silence marker packet instead.
- **Deliverables**:
  - Silence marker packet (empty data, valid timestamp + duration)
  - Jitter buffer handles silence markers without stuttering
- **Notes**: OpusPacket::silence() constructor, is_silence() check. Decoder outputs zero-filled PCM for silence markers.

### P3.9.5: Audio RAII Guards (R-H4)
- **Status**: `[x]`
- **Description**: Audio threads can leak if `run_host()`/`run_join()` return early. Implement `Drop` on audio handle structs or use scoped guards.
- **Notes**: Drop impls on AudioCaptureThread, AudioEncodeThread, AudioDecodeThread, AudioPlaybackThread.

### P3.9.6: Network Allocation Limits (R-H3)
- **Status**: `[x]`
- **Description**: `recv_message` allocates up to 4MB per message from untrusted network data. Add per-connection concurrent allocation limits.
- **Notes**: Reduced MAX_MESSAGE_LEN from 4MB to 2MB. Accommodates clipboard (1MB) with overhead.

### P3.9.7: Keepalive Handling in Decoder (R-H5)
- **Status**: `[x]`
- **Description**: Decode thread doesn't check `is_keepalive` — empty data packets from the keepalive path could crash FFmpeg. Add guard in decode loop.
- **Notes**: Skip is_keepalive packets before feeding to FFmpeg decoder.

### P3.9.8: Workspace Dependency Consistency (R-H7)
- **Status**: `[x]`
- **Description**: Some crates use `path = "../..."` instead of `workspace = true`. Standardize all internal crate references.
- **Notes**: All internal crate refs now use workspace = true.

### P3.9.9: Frame Drop Counters (R-L2)
- **Status**: `[x]`
- **Description**: `try_send` failures are only logged at `debug`. Add `AtomicU64` counters for dropped frames, exposed via diagnostics API or periodic `info!` logs.
- **Notes**: DropCounter in pairwire_core::diagnostics. Wired into capture thread and moq video drain. Logged at powers-of-2 and every 100th drop.

### P3.9.10: Minimize tokio Features (R-L8)
- **Status**: `[x]`
- **Description**: Workspace uses `features = ["full"]` for tokio. Audit and enumerate only needed features to reduce compile time and binary size.
- **Notes**: Replaced with rt, rt-multi-thread, sync, time, macros. Dropped net, io-util, signal, fs.

### P3.9.11: Protocol Boundary Tests (R-L5)
- **Status**: `[x]`
- **Description**: No adversarial/boundary tests for protocol types (MAX values, empty collections, truncated input). Add fuzzing and property-based tests.
- **Notes**: 36 boundary tests in boundary_tests.rs covering all protocol types.

---

## Phase 4: Terminal Sharing + Localhost Tunneling

> **Goal**: Share terminal sessions natively via PTY forwarding AND tunnel localhost ports over the existing P2P connection. Together these complete the "I can see your code, run your app, and use your services" story.
> **Estimated effort**: 4-5 weeks
> **Docs**: `docs/06-competitive-analysis.md`, `docs/07-ux-design.md`, `docs/09-localhost-tunneling.md`

### P4.1: PTY Capture & Multi-Session Management
- **Status**: `[ ]`
- **Description**: Capture terminal output by creating PTYs and reading their output streams. Support multiple concurrent terminal sessions (one running the server, one for tests, one for git).
- **Deliverables**:
  - `TerminalSession { id: u8, shell, cwd, permission: ReadOnly | ReadWrite }` — managed by host
  - Create PTY with configurable shell (detect `$SHELL` or explicit path)
  - Read PTY output as byte stream
  - Handle terminal resize (SIGWINCH)
  - `TerminalFrame { session_id, data, timestamp }` — output chunk
  - `TerminalOpen / TerminalClose / TerminalResize` control messages
  - Max 8 concurrent sessions
  - **State snapshot on join**: When guest connects mid-session, send the current screen buffer (maintained via `vte` crate) so they don't see a blank terminal
- **Acceptance criteria**:
  - Test: spawn PTY, send "echo hello", receive "hello" in output
  - Multiple sessions can run concurrently
  - Resize propagation works
  - Late-joining guest sees existing terminal content
  - `just all` passes

### P4.2: Terminal Streaming Over QUIC
- **Status**: `[ ]`
- **Depends on**: P4.1, P1.2
- **Description**: Stream terminal I/O over a dedicated reliable QUIC stream per session. Bidirectional: host output → guest display, guest input → host PTY (when permitted).
- **Deliverables**:
  - One QUIC bidi stream per terminal session (priority 1 — same as input)
  - Stream type tag `0x54` ('T') for terminal streams (via `UniStreamRouter` pattern)
  - `TerminalData { session_id, direction: Output|Input, data }` framing
  - Permission enforcement: host controls read-only vs read-write per session
  - Guest input only forwarded when session is read-write AND guest has control (P3.5.2)
- **Acceptance criteria**:
  - End-to-end: guest sees host's terminal output in real-time
  - Guest can type into read-write sessions
  - Read-only sessions reject guest input
  - `just all` passes

### P4.3: Terminal Renderer — Two Paths
- **Status**: `[ ]`
- **Depends on**: P4.2
- **Description**: Two ways for guests to access shared terminals, serving different user types.
- **Path A — Native terminal (power users)**:
  - `pairwire terminal attach <session_id>` bridges the QUIC stream to a local PTY
  - Opens in the user's real terminal emulator — their fonts, keybinds, tmux, everything
  - This is how SSH works and it's the primary recommended path
  - Implementation: spawn a local PTY, pipe QUIC stream bytes to/from it
- **Path B — In-viewer terminal panel (GUI users)**:
  - Split view in the egui viewer: screen share on top/left, terminal panel on bottom/right
  - Terminal rendering via `alacritty_terminal` or `vte` crate for ANSI parsing
  - Support 24-bit color, Unicode, cursor, scrollback (10k lines default)
  - Good enough for watching logs and light interaction, not meant to replace a real terminal
  - Tab bar for switching between terminal sessions
- **Acceptance criteria**:
  - Path A: `pairwire terminal attach 1` opens a working terminal in the user's shell
  - Path B: Terminal panel in viewer renders ANSI output correctly (colors, cursor)
  - Both paths work simultaneously (one user on Path A, another on Path B)
  - `just all` passes

### P4.4: Tunnel Protocol & Host-Side Proxy
- **Status**: `[ ]`
- **Depends on**: P1.2
- **Description**: Define tunnel protocol types and implement the host-side proxy that connects incoming QUIC streams to localhost ports.
- **Docs**: `docs/09-localhost-tunneling.md`
- **Deliverables**:
  - Protocol types in `crates/protocol/`:
    - `TunnelOffer { ports: Vec<TunnelPort> }` — host advertises available ports during handshake
    - `TunnelRequest { tunnel_id: u16, host_port: u16, guest_bind_port: u16 }` — guest requests a tunnel
    - `TunnelAccept { tunnel_id: u16 }` / `TunnelReject { tunnel_id: u16, reason }` — host responds
    - `TunnelClose { tunnel_id: u16 }` — either side tears down
  - `TunnelManager` on host:
    - Configured via `--tunnel 3000,5432` or config file `default_ports`
    - Accepts QUIC bidi streams tagged `0x4C` ('L' for localhost)
    - For each stream: connect to `127.0.0.1:<host_port>`, proxy bytes bidirectionally
  - Security: database ports (3306, 5432, 6379, 27017) blocked by default (configurable)
  - Host must approve tunnel requests (auto-accept configurable in `permissions.auto_accept_tunnels`)
- **Acceptance criteria**:
  - Host offers port 3000, guest requests tunnel, QUIC stream opens, bytes flow to localhost:3000
  - Database port blocked by default, allowed with explicit config
  - `just all` passes

### P4.5: Tunnel Guest-Side Listener & Bidirectional Proxy
- **Status**: `[ ]`
- **Depends on**: P4.4
- **Description**: Guest-side TCP listener that binds local ports and proxies to the host via QUIC.
- **Deliverables**:
  - `TunnelClient` on guest:
    - For each accepted tunnel: bind `127.0.0.1:<guest_bind_port>` as TCP listener
    - Each incoming TCP connection opens a new QUIC bidi stream to host
    - `tokio::io::copy_bidirectional` for byte proxying (~50 lines per direction)
  - Handle multiple concurrent TCP connections per tunnel (browser opens 6 connections)
  - Graceful cleanup: TCP close → QUIC stream finish, QUIC stream close → TCP reset
  - Port conflict on guest: try requested port, if taken auto-pick and warn user
- **Acceptance criteria**:
  - Guest opens browser to `localhost:3000`, sees host's dev server
  - Multiple concurrent connections work (e.g., browser + API client)
  - WebSocket upgrade works through the tunnel
  - `just all` passes

### P4.6: Tunnel Management CLI & UI
- **Status**: `[ ]`
- **Depends on**: P4.5
- **Description**: CLI commands and session bar panel for managing tunnels.
- **Deliverables**:
  - CLI: `pairwire tunnel ls`, `pairwire tunnel add <port>`, `pairwire tunnel stop <port>`
  - Host auto-detection mode (optional): monitor listening sockets, offer new ones
  - Session bar tunnel panel: list active tunnels with status indicators
  - Startup output: "Binding localhost:3000 → host:3000 ✓"
- **Acceptance criteria**:
  - `pairwire tunnel ls` shows active tunnels with status
  - Tunnels can be added/removed during a live session
  - `just all` passes

### P4.7: Settings & Config System
- **Status**: `[ ]`
- **Description**: Persistent configuration with three-layer merge: CLI flags > config.toml > defaults.
- **Docs**: `docs/07-ux-design.md` (settings section)
- **Deliverables**:
  - `Settings` struct in `crates/core/` that merges all layers
  - Config file at `~/.config/pairwire/config.toml` (platform-correct via `directories` crate)
  - TOML format, human-editable
  - Sections: identity, capture, audio, permissions, terminal, tunnels, ui
  - **Audio quality knobs** (user-overridable ceiling): `audio.bitrate_kbps` (default: 64 — 32 kbps showed artifacting in real-device test), `audio.channels` (default: mono)
  - **Video quality**: user config provides a ceiling (max_fps, max_resolution, h264_profile); actual operating point is negotiated — see P4.7.1
  - CLI: `pairwire config edit` (opens in `$EDITOR`), `pairwire config get <key>`, `pairwire config set <key> <value>`
  - Safe defaults: remote_control=denied, clipboard_sync=text_only, terminal_access=read_only, database_ports=blocked
- **Acceptance criteria**:
  - Config file created on first run with commented defaults
  - CLI flag overrides config file value
  - `pairwire config set capture.fps 60` persists correctly
  - `just all` passes

### P4.7.1: Intelligent Quality Negotiation
- **Status**: `[ ]`
- **Depends on**: P2.2, P4.7
- **Description**: The encode resolution and quality profile should not be a static default — they should be derived from both peers' capabilities and the joiner's actual viewer size. Encoding at the host's native Retina resolution when the guest's viewer is 1440p is pure bandwidth waste and causes unnecessary downscaling artifacts on the receive side.
- **Design**:
  - **Handshake already exchanges display info** (resolution, scale factor) — extend it to also exchange:
    - Viewer window size (joiner sends its current window dimensions)
    - Hardware encoder/decoder availability (VideoToolbox, NVENC, VA-API, etc.)
    - Max acceptable resolution (from user config ceiling if set)
  - **Host computes encode resolution** as: `min(host_native_res, joiner_viewer_res, config_ceiling)`, rounded to a clean H.264-friendly size (divisible by 2)
  - **Joiner viewer resize** → sends a `ViewerResize` control message → host recomputes and reinitializes encoder at new resolution. This means the encode resolution tracks the viewer, not the source display.
  - **H.264 profile selection**: if both sides have hardware encode/decode, use `High` profile (B-frames, 8x8 DCT — noticeably crisper). Fall back to `Main`, then `Constrained Baseline` only if hardware is absent.
  - **Initial quality tier**: start at the best tier the network and hardware support, not always at the lowest. P2.5 (`QualityMonitor`) handles degradation; this task sets the right starting point.
  - **Audio**: both peers advertise their mic sample rate and encoder capabilities; pick the best mutually supported bitrate (cap at 64 kbps mono for voice, allow 96 kbps stereo if system audio is shared).
- **Deliverables**:
  - Extended `Handshake` type with `viewer_size`, `hw_encoder`, `hw_decoder`, `config_caps` fields
  - `negotiate_quality(host_caps, guest_caps) -> NegotiatedQuality` pure function (easy to test)
  - `ViewerResize { width: u32, height: u32 }` control message → triggers encoder reconfig
  - Host uses `NegotiatedQuality` to initialize encoder thread instead of hardcoded values
  - `NegotiatedQuality` logged at session start so it's visible in traces
- **Notes**: First real-device test (2026-04-01) showed "not crisp" video — root cause was Constrained Baseline profile + 4K→1080p downscale happening in the encoder rather than matching the viewer. Also audio artifacting at 32 kbps.
- **Acceptance criteria**:
  - `negotiate_quality` unit tests covering: Retina host + small viewer → encode at viewer size; both have HW → High profile; neither has HW → Baseline
  - Resize viewer window → encoder reinitializes at new resolution within one keyframe interval
  - `just all` passes

---

## Phase 5: Multi-Party & Polish

> **Goal**: Expand from 1:1 to small-group collaboration (mob programming, team reviews). This is the natural evolution after the core 1:1 experience is solid.
> **Docs**: `docs/08-multi-party-architecture.md`
>
> **Topology model** (2026-04-02):
> - `n=2` (1:1): iroh P2P direct. Zero server cost. Free tier.
> - `n≥3`: moq-relay. Server infrastructure required. Paid tier.
>
> QUIC is TLS 1.3 end-to-end — the relay forwards encrypted streams it cannot read. Privacy is preserved regardless. The relay is purely a routing/fan-out node.
>
> The 1:1 free tier is the acquisition funnel. Multi-party relay is the moat — not just because of server costs, but because relay infrastructure enables features that can't exist in P2P: session recording, live transcription (Whisper), replay, analytics, team management, admin controls. These are the differentiated product surface. The CDN-like relay network (global PoPs for low latency) is the infrastructure moat.
>
> **Session types:**
> - **Direct session** (`pairwire://`): P2P invite, 1 guest max, free forever
> - **Room session** (`pairwire-room://` or URL): relay-backed invite, N guests, paid tier
>
> When a host currently in a 1:1 P2P session wants to add a second guest, they generate a room invite — existing guest and new guest both reconnect to the relay. This is a deliberate session upgrade, not a transparent migration.

### P5.1: Multi-Party — Room Sessions via moq-relay
- **Status**: `[ ]`
- **Depends on**: P2.1.1
- **Description**: Introduce "room sessions" backed by moq-relay (moq-dev/moq binary). Host connects to relay and publishes screen/audio as hang broadcasts. Guests connect to the relay and subscribe. The relay handles fan-out — host upload is 1× bitrate regardless of guest count.
- **Docs**: `docs/08-multi-party-architecture.md`
- **Topology**:
  ```
  Host → moq-relay (pairwire-operated, CDN-like) → Guest A
                                                  → Guest B
                                                  → Guest C
  Control/cursors/reactions → relay-forwarded datagrams (tiny, latency-sensitive)
  ```
- **Why relay from the start (not star)**:
  - Host upload stays 1× (not N×) — removes the bandwidth bottleneck entirely
  - Relay enables paid-tier features: recording, transcription, analytics — these attach to the relay, not the host
  - moq-relay is a deployable binary from moq-dev/moq, not something to build
  - hang adoption (P2.1.1) means the host already speaks the relay's protocol
- **Session types introduced**:
  - **Direct session**: 1:1 iroh P2P (existing). CLI: `pairwire host` / `pairwire join <invite>`.
  - **Room session**: relay-backed, N guests. CLI: `pairwire room` / `pairwire join <room-invite>`. Invite URL points at relay, not host directly.
- **Deliverables**:
  - `RoomSession` — host connects to configured relay endpoint, publishes screen + audio via hang, opens control/datagram channels through relay
  - Room invite format: `pairwire-room://<relay-host>/<room-id>?token=<jwt>` (relay authenticates via JWT)
  - Peer list: each peer gets `peer_id: u8`, assigned color, display name
  - Cursors/reactions/drawing forwarded via relay datagrams
  - Control: one guest drives at a time
  - Slow guest isolation: relay's per-subscriber flow control handles this (host is not affected)
  - Default cap: 8 guests (relay-enforced via room config)
- **Acceptance criteria**:
  - 3 guests connect to a room and see host's screen simultaneously
  - Host upload stays ~constant as guests are added (verify with bandwidth monitor)
  - Each guest has distinct cursor color
  - Only one guest has control at a time
  - `just all` passes

### P5.2: Multi-Party — Mesh Audio
- **Status**: `[ ]`
- **Depends on**: P5.1
- **Description**: Upgrade audio from star (host→all) to mesh (everyone hears everyone). Each peer connects directly to every other peer for audio.
- **Deliverables**:
  - After initial star connection, guests discover each other via host relay
  - Each peer pair establishes a direct iroh connection for audio (if NAT allows)
  - Falls back to host relay for peers that can't connect directly
  - Audio mixing: each peer receives N-1 audio streams, mixes locally
  - Mute indicators per peer
- **Acceptance criteria**:
  - 3+ peers can all hear each other
  - Direct connections used where possible (verify via logs)
  - `just all` passes

### P5.3: Relay Infrastructure — Deferred (Post-Launch)
- **Status**: `[ ]`
- **Depends on**: P5.1
- **Description**: Design and build out the relay network that powers room sessions. **Deliberately deferred** — figure this out after the core product is built, packaged, and explored.
- **What moq-relay actually supports (researched 2026-04-02)**:
  - Clustering is **live and shipped** — `cdn.moq.dev` runs 3 nodes today (US-Central, EU-Central, Southeast Asia)
  - **Not gossip.** Discovery via a root node using moq-lite pub/sub (the author calls it "dog-fooding"). Each leaf registers itself by publishing an empty broadcast at `internal/origins/<hostname>`. Other leaves discover peers by watching that prefix on the root.
  - **Media is leaf-to-leaf**: root only handles the announcement directory. EU subscriber → discovers stream is on US leaf → EU leaf opens a direct QUIC connection to US leaf. Max 2 media hops. Root is never in the media path.
  - **NxN full mesh** between leaf nodes. Recommended 3-5 nodes with current implementation.
  - **Single root only** — root failure doesn't break existing sessions (media is leaf-to-leaf) but prevents new session discovery. Multi-root and tree topology are planned but unshipped (moq-dev/moq#927). Until that lands, keep the root on a reliable VM; a second node per region adds nothing without intra-region replication. Revisit when #927 ships — tree topology would allow regional sub-roots aggregating local leaves, which is when 2-nodes-per-region makes sense.
  - JWT auth with a `cluster: bool` claim distinguishes relay-to-relay connections from client connections.
  ```
  Root (announcement directory only — a regular moq-relay with no upstream configured)
  ├── Leaf US  ←→  Leaf EU  ←→  Leaf APAC    (full mesh, direct QUIC)
       ↑                ↑
    publisher         subscriber
  Media: publisher → US leaf → EU leaf → subscriber  (2 hops max, root not involved)
  Announcements: leaves publish to root, leaves watch root for peer discovery
  ```
- **What to think about when we get here**:
  - **Option A — Cloudflare's relay first**: Cloudflare operates a public MoQ relay on their anycast network (`relay.cloudflare.mediaoverquic.com`). Same protocol, global coverage, zero ops. Could be the right default before pairwire has scale to justify running its own nodes. Downside: less control over paid-tier feature attachment (recording, transcription).
  - **Option B — self-hosted moq-relay**: 3 nodes (US, EU, APAC) matches the live `cdn.moq.dev` topology. Terraform module is already in the moq-dev/moq repo. GCP/Linode or similar. Gives full control for attaching recording, Whisper, analytics workers to the relay.
  - Likely path: start with Cloudflare for early paid tier, migrate to self-hosted as volume and feature requirements justify it.
  - Room invites encode the relay URL; switching relay providers is a config change, not a protocol change.
  - Node failure mid-session: reconnect to same leaf (if recovered) or reroute via another leaf.
  - Paid-tier features at the relay layer: recording (tap hang tracks → object storage), live transcription (forward `audio/asr` simulcast track to Whisper worker), analytics, team room management.
  - Self-hosted path: moq-relay is open source; teams run their own node(s) pointed at by pairwire config.
  - Tree topology (moq-dev/moq#927): when shipped, enables 2 nodes per region with a regional sub-root. Not available yet — revisit.
- **Not blocked on anything in the codebase** — P5.1 client works against any moq-relay node. This task is infra + ops + paid-tier product decisions.
- **Acceptance criteria**: TBD when we revisit post-launch.

### P5.4: AI-Assisted Pairing
- **Status**: `[ ]`
- **Description**: Shared LLM context in the pairing session. "Ask the AI about this code" as a first-class feature. Could use screen content + terminal history as context.

### P5.5: Editor Integration (LSP-Aware Cursors)
- **Status**: `[ ]`
- **Description**: Share editor state at the semantic level (LSP events, cursor position in file) alongside screen sharing.

### P5.6: Recording (Save Session to File)
- **Status**: `[ ]`
- **Description**: Record screen + audio to a local file during a session. Mux H.264 video + Opus audio into MP4/MKV. Terminal output saved as asciicast (compatible with asciinema). Lower priority — focus on live collaboration first.

### P5.7: Rich Reactions — Custom Animated Stickers with Sound
- **Status**: `[ ]`
- **Description**: Go beyond static emoji. Custom reactions that are short animated clips with audio — think Discord stickers meets Twitch alerts, but for pair programming.
- **Format concept**: `.pwreact` — a lightweight reaction bundle:
  - **Visual**: Animated WebP or APNG (transparent background), max 128x128px, max 2s loop
  - **Audio**: Short Opus clip (optional), max 2s, auto-mixed at low volume so it doesn't interrupt voice chat
  - **Metadata**: name, author, category, trigger shortcode (e.g., `:shipit:`)
  - Bundle is just a zip with `manifest.toml` + assets. Small enough to send inline via QUIC stream on first use, then cached by peer.
- **Why not GIF/video?**: GIFs are huge and have no alpha. Video codecs (VP9/AV1 with alpha) are overkill for 128px 2s clips. WebP/APNG give transparency + animation + tiny file size. The audio channel is the differentiator — a quick "ding!" or airhorn or applause makes reactions feel alive.
- **UX flow**:
  - Built-in pack ships with the app (8-12 reactions: applause, mind-blown, ship-it, bug-found, etc.)
  - Users can import `.pwreact` packs (drag-and-drop or CLI `pairwire react install <path>`)
  - Teams can share packs via URL (hosted or P2P via iroh)
  - Reaction picker in viewer: grid of animated thumbnails, search by shortcode
  - Hotkey: type `:shortcode:` in a reaction input bar, or number keys for favorites
- **Wire protocol**:
  - First use of a custom reaction: sender transmits the bundle inline (~10-50KB) via reliable stream
  - Subsequent uses: just the reaction ID (datagram, <20 bytes) — peer already has it cached
  - `ReactionBundle { id: u64, name, visual: Bytes, audio: Option<Bytes> }` — stream
  - `CustomReaction { peer_id, bundle_id: u64, x, y }` — datagram
- **Stretch considerations**:
  - Could support transparent video (VP9 alpha / AV1 alpha) for higher-quality animations if bundle size budget allows (~200KB)
  - Could support "reaction GIFs" — short looping clips from a URL, rendered with transparency
  - Sound reactions could be positional (play from the cursor location in stereo) for fun spatial feel

---

## Phase 6: Advanced Capture & Display Options

> **Goal**: Power-user and niche capture/display features. Build only after the core feature set has GUI coverage and the 1:1 experience is polished. Each of these is self-contained but non-trivial.
> **Prerequisites**: Phase 3.5 complete, P4.7 (config system) in place, GUI surfaces for core features exist.

### P6.1: Region / Partial Screen Sharing
- **Status**: `[ ]`
- **Description**: Host selects a rectangular region of their display to share instead of the full screen. The capture pipeline crops pre-encode, so the guest never receives pixels outside the region. Useful for sharing a single editor pane while keeping the rest of the desktop private.
- **UX**: A draggable/resizable overlay appears on the host's screen to define the crop region. Region persists for the session; host can adjust live. A "full screen" shortcut resets it.
- **Implementation notes**: scap captures the full display; apply a crop rect before the frame enters the encode thread. The encode resolution becomes the region size, not the display size — feeds naturally into P4.7.1's negotiated resolution. Region coordinates need to account for Retina scale factor.
- **Deliverables**:
  - Region selection UI (overlay window on host)
  - `CaptureRegion { x, y, width, height }` in `CaptureConfig`
  - Crop applied in the encode thread before scaler, not in scap
  - Region adjustable mid-session (triggers keyframe + `ViewerResize` control message)
  - Config persistence: `capture.region` saved per-host-display by display UUID

### P6.2: Window Sharing
- **Status**: `[ ]`
- **Description**: Host shares a single application window rather than a full display. The capture follows the window as it moves, resizes, or goes in/out of full-screen. Guest sees only that window, not the surrounding desktop.
- **Implementation notes**: scap supports window-level capture via `Target::Window`. The complication is tracking window geometry changes — on macOS, use `CGWindowListCopyWindowInfo` to poll or `NSWorkspace` notifications to detect moves/resizes, then update the capture target or crop rect accordingly. Window minimisation or occlusion should pause the stream (send a "paused" control message) rather than sending garbage frames.
- **Deliverables**:
  - Window picker UI: list of visible windows with app icon + title
  - Live tracking of window position/size changes
  - Pause/resume stream on minimise/restore
  - `CaptureTarget::Window(window_id)` variant in `CaptureConfig`

### P6.3: Multi-Monitor — Host Display Selection
- **Status**: `[ ]`
- **Description**: When the host has multiple displays, let them choose which one to share. Switchable mid-session without disconnecting.
- **Implementation notes**: scap exposes `Target::Display(display_id)`. Enumerate displays with `CGGetActiveDisplayList`. On switch, tear down the current capture thread and restart with the new display — this triggers a new keyframe group so the guest's decoder resets cleanly. The new display may have a different resolution, so P4.7.1's `ViewerResize` flow handles the encode resolution update.
- **Deliverables**:
  - Display enumeration (name, resolution, whether it's the main display)
  - Display picker in host UI / menu bar
  - Mid-session switch: clean teardown → restart → guest sees new display within one keyframe interval
  - `CaptureTarget::Display(display_id)` variant in `CaptureConfig`

### P6.4: Multi-Monitor Viewer — Span Across Guest Screens
- **Status**: `[ ]`
- **Description**: When the guest has multiple displays and the host is sharing a multi-monitor setup (or a very wide display), allow the guest's viewer to span across their own screens to use the full real estate. The viewer becomes a borderless window that can be dragged across display boundaries, with the OS compositor handling the spanning.
- **Notes**: This is genuinely unusual — most screen share tools don't attempt this. The core challenge is that egui/wgpu windows are per-monitor; spanning requires either a single oversized window (works on macOS if you position it across the boundary) or multiple synchronised windows (harder). The simpler path: allow the viewer window to be positioned and resized freely across display boundaries and let the OS handle compositing. The trickier path (separate windows per guest display, each showing a different crop of the shared content) is only worth it if multi-monitor host sharing (P6.3 extended to share all displays simultaneously) is also implemented.
- **Deliverables**:
  - Viewer window can be freely resized across display boundaries (no artificial single-display constraint)
  - Stretch goal: "span mode" that snaps the viewer to fill all guest displays automatically
  - Stretch goal: host can share all displays simultaneously as one logical canvas; guest viewers map their screen layout to the host's layout

### P6.5: Privacy Zones (Pre-Encode Redaction)
- **Status**: `[ ]`
- **Description**: Host defines rectangular regions that are blurred or blacked out before encoding. The redacted pixels are never transmitted — they're zeroed in the raw frame buffer before the encode thread sees them. Designed for scenarios where credentials, private messages, or confidential content are visible on screen.
- **Notes**: Applied before the scaler/encoder in the capture pipeline, not as a renderer overlay. This is a security property — overlay redaction that happens post-encode is not safe. Regions should be configurable per-application (e.g. "always redact the rectangle where 1Password sits") using window geometry as an anchor.
- **Deliverables**:
  - `PrivacyZone { x, y, width, height, mode: Blur | BlackOut }` in `CaptureConfig`
  - Applied in encode thread to raw frame buffer before encode
  - UI for defining zones (drag to create, click to remove)
  - Optional: auto-detect zones by window name (e.g. add any window titled "1Password" automatically)
  - Zones persist in config by display UUID

### P6.5.1: Extensible Privacy Detection — Plugin Architecture
- **Status**: `[ ]`
- **Depends on**: P6.5
- **Description**: P6.5 handles static user-defined rectangles. This extends it with a detection layer: rules and plugins that automatically identify *what* to redact, without the user having to manually draw boxes. A separate project in its own right — capture here for design intent before implementation.

---

#### The Core Tension

Automatic redaction requires *analysing* frames to decide what to hide. If that analysis happens outside the process, you are sending unredacted frames to a third party to decide what to redact — which is precisely the thing you're trying to prevent. This is the fundamental design constraint that shapes everything below.

**Resolution**: Detection always runs on unredacted frames. The trust boundary must therefore be:
- **Local-only by default** — detection runs in-process or in a sandboxed sidecar on the host machine. No frames leave the machine for detection.
- **External services are opt-in and explicit** — user consciously chooses to send frames to an external service, understands the tradeoff. Suitable for enterprise deployments with a trusted internal service, not a default.

---

#### Detection Layers (in order of complexity)

**Layer 1 — Window identity rules** (cheapest, no frame analysis needed)
  - Match on OS window metadata: app bundle ID, window title, process name
  - "WhatsApp begone": if a window matching `com.facebook.WhatsApp` or `WhatsApp` is visible within the capture region, redact its entire bounding rect
  - Uses `CGWindowListCopyWindowInfo` (macOS) — no frame processing at all
  - Rule format: `{ match: { bundle_id: "com.facebook.WhatsApp" }, action: Blackout, message: "WhatsApp begone — for your eyes only" }`
  - Also handles: 1Password, banking apps, Slack DMs (title-based), terminal windows with known credential patterns

**Layer 2 — Local frame analysis via Apple Vision / on-device ML** (medium cost, fully private)
  - OCR the frame using `Vision.framework` (macOS) to extract text regions
  - Pattern-match against PII signatures: credit card numbers (Luhn), SSNs, API key patterns, email addresses in form fields
  - Face detection: optionally blur faces (useful for sharing a screen with video calls in the background)
  - Apple Vision runs entirely on-device, no network, hardware-accelerated on Apple Silicon
  - Performance budget: run at a fraction of capture FPS (every 10th frame is fine — redaction zones are stable for seconds at a time). Cache results between analysis frames.
  - Output: list of `DetectedRegion { rect, category: CreditCard | SSN | ApiKey | Face | ... , confidence }` → applied as redaction zones

**Layer 3 — WASM plugin interface** (extensible, sandboxed)
  - A plugin receives a downsampled/low-res version of the frame (e.g. 480p) as raw bytes + window metadata
  - Returns a list of regions to redact (coordinates in normalised 0.0-1.0 space)
  - WASM sandbox: plugin cannot make network calls, cannot access filesystem, cannot see anything except the frame it's given
  - Enables: community-built detectors, enterprise custom rules, specialised PII patterns for specific industries
  - Plugin format: `.pwprivacy` bundle — WASM module + `manifest.toml` (name, description, version, permissions)
  - Built-in plugins ship with pairwire: `pii-detector`, `window-classifier`, `face-blur`

**Layer 4 — External HTTP service** (opt-in only, explicit user consent required)
  - Plugin that POSTs frame data to a configurable endpoint, receives redaction regions back
  - Gated behind an explicit config flag: `privacy.external_detection.enabled = true` + `privacy.external_detection.url`
  - Consent dialog on first enable: "You are sending screen frames to [url]. These frames are unredacted. Are you sure?"
  - Designed for: enterprise deployments with an internal detection service, specialist providers (DLP vendors, etc.)
  - Frame sent is the full capture frame — the service sees everything. This is intentional and documented.

---

#### Personality Layer ("Opal-style quirk")

Rules can carry a human-readable `message` that surfaces in the host UI when the rule fires. Not a warning — a character moment. Examples:
  - `"WhatsApp begone — this is between you and your group chat 🫣"`
  - `"1Password is doing its thing. Your secrets are safe."`
  - `"Detected a credit card number. Redacted. You're welcome."`
  - `"Face detected in background — blurred. They didn't ask to be on screen."`

The message appears as a transient toast on the host's screen (not the guest's — the guest just sees the redaction). Optional, dismissable, configurable off. The goal is to make privacy feel intentional and even a bit delightful rather than a bureaucratic compliance checkbox.

Rule names in config are also human-readable: `[[privacy.rules]]` entries have a `name` field. "WhatsApp Begone", "No PII in Forms", "Keep Faces Private". These show up in the host UI as toggleable named rules, not anonymous rectangles.

---

#### Architecture Sketch

```
CaptureThread → raw frame
    │
    ├─→ PrivacyEngine (async, runs every Nth frame)
    │       ├─ WindowRuleEvaluator   (Layer 1 — sync, O(windows))
    │       ├─ LocalMLDetector       (Layer 2 — async, Vision.framework)
    │       ├─ WasmPluginRunner      (Layer 3 — sandboxed, parallel)
    │       └─ ExternalServiceClient (Layer 4 — opt-in HTTP, async)
    │       → emits: Vec<RedactionZone>  (cached, updated every Nth frame)
    │
    └─→ EncodeThread
            applies RedactionZone list to raw frame buffer before encode
            (same mechanism as P6.5 static zones)
```

The `PrivacyEngine` runs out-of-band from the encode thread. It analyses frames asynchronously and publishes an updated `Vec<RedactionZone>` via a `watch::Sender`. The encode thread reads the latest snapshot each frame — if detection lags by a frame or two, the zones from the previous analysis are applied (conservative: zones persist until explicitly cleared, not until re-confirmed).

---

#### What Makes This a Separate Project

- WASM runtime integration (likely `wasmtime`) is a non-trivial dependency
- Apple Vision framework bindings are not mature in the Rust ecosystem — likely needs an Objective-C/Swift bridge
- The plugin distribution story (discovery, installation, trust, versioning) is a product in itself
- External service protocol needs a security review before shipping

**Recommended sequencing**: Ship P6.5 (static zones) + Layer 1 (window rules) first — covers 80% of real use cases with minimal complexity. Layer 2 (Vision OCR) is the highest-value next step. Layers 3-4 are post-1.0.

- **Acceptance criteria** (for Layer 1 + 2 MVP):
  - Window rule fires within one capture frame of the target window becoming visible
  - OCR-based PII detection correctly redacts a visible credit card number in a form field (test with a fixture image)
  - Detection runs without dropping encode FPS (budget: <5ms per analysis frame on Apple Silicon)
  - `just all` passes

### P6.6: Cursor-Follow / Auto-Pan Mode
- **Status**: `[ ]`
- **Description**: When the host has a large or high-DPI display and the guest's viewer is small, instead of showing the full screen scaled down (losing readability), the viewer crops to a region around the host's cursor and pans smoothly as the cursor moves. Guest sees a 1:1 or close-to-1:1 crop that follows the action.
- **Notes**: This is viewer-side but requires host cooperation: the host's cursor position (already sent via `CursorUpdate` datagrams in P3.5.1) drives the pan on the guest side. The host encodes the full screen at full resolution (or at least a high resolution); the guest renders a crop of the decoded frame. No encode-side changes needed — this is purely a viewer render transform. The crop rect is computed from cursor position + viewer window size, with easing to avoid jarring jumps.
- **Deliverables**:
  - "Follow cursor" toggle in viewer UI
  - Smooth pan with configurable easing (lerp toward cursor, not instant jump)
  - Dead zone: don't pan if cursor is within the centre N% of the viewport
  - Zoom level configurable (default: fit host resolution to viewer, i.e. same as current; 1:1 is the useful new mode)

---

## Phase W: Web Viewer ("Send a Link")

> **Goal**: Anyone can watch a pairwire session from a browser with zero install. No native app, no account, no friction. Send a link, they're in.
>
> **Scope deliberately limited**: the web viewer is a **viewer + pointer**, not a full peer. It can watch, point, draw, react, and hear. It cannot drive (no keyboard/mouse injection), cannot access the terminal, and cannot use tunnels. This is a feature, not a limitation — it's the right UX for reviewers, stakeholders, and rubber ducks.
>
> **Can be built in isolation**: this phase has no dependencies on Phase 4-6 features. It can be started as soon as P3.5.1 (cursor) and P3.1/P3.2 (audio) are working. Good candidate for a focused side sprint.
>
> **Prerequisite**: P2.1.1 (hang adoption) must be complete before PW.3. hang's frame format is WebCodecs-native — PW.3's `VideoDecoder` and PW.4's `AudioDecoder` can consume hang frames directly without custom header parsing. Starting PW.3 before hang adoption means writing a custom parser you'll have to delete later.
>
> **Estimated effort**: 6-8 weeks (gateway bridge + web client)

---

### Architecture

```
Native host process
    │
    ├── iroh endpoint          ← existing, unchanged
    │       ↕ iroh QUIC
    │   Native guest (existing)
    │
    └── WebTransport bridge    ← new, spun up with --web flag
            ↕ WebTransport (HTTP/3 over QUIC)
        Browser guest (new web client)
```

The bridge is **not a relay** — it runs inside the host process (or as a sidecar spawned by it). It does not transcode video. It repackages already-encoded H.264 NAL units and Opus frames from the internal channels into the format the browser expects. Cursor/reaction/draw datagrams are forwarded as-is. The host encodes once regardless of how many web guests are watching.

The hosted web app at `join.pairwire.app` is a static file server only — it serves the JS/WASM bundle. All session data flows directly between the browser and the host's bridge. No pairwire server sees session content.

---

### PW.1: WebTransport Bridge (Host Side)
- **Status**: `[ ]`
- **Depends on**: P1.10, P3.1
- **Description**: An optional WebTransport server that runs inside the host process when `--web` is passed. Accepts browser connections, authenticates via session token, and bridges the internal media/datagram channels to WebTransport streams.
- **Crate**: new `crates/web-bridge` — keeps the HTTP/3 dependency isolated from the rest of the workspace
- **Deliverables**:
  - `WebBridge` struct: starts a `wtransport` (or `h3`) server on a configurable local port (default: 9443)
  - TLS: self-signed cert generated at startup, cert hash included in the web invite URL (browsers accept this via the `serverCertificateHashes` WebTransport option — no CA required)
  - Session token auth: browser must present the token from the invite URL in the first datagram; reject anything else
  - Bridges from internal channels:
    - `broadcast::Receiver<EncodedPacket>` → WebTransport unidirectional stream (one stream per keyframe group, mirrors the native screen stream structure)
    - `broadcast::Receiver<OpusFrame>` → WebTransport unidirectional stream
    - `watch::Receiver<CursorUpdate>` → WebTransport datagram
  - Accepts from browser:
    - `CursorUpdate` datagrams → forwarded to host's existing cursor channel
    - `DrawPoint` / `DrawStroke` datagrams → forwarded to drawing channel
    - `Reaction` datagrams → forwarded to reaction channel
  - `WebGuestConnected` / `WebGuestDisconnected` events emitted to session so host UI can show web guest count
  - Max web guests: configurable (default: 4)
- **Dependency note**: `wtransport` crate is the leading Rust WebTransport server implementation. Evaluate at task start — if immature, `quinn` with a WebTransport framing layer is the fallback.
- **Acceptance criteria**:
  - `cargo test -p web-bridge` passes without a browser (mock WebTransport client in tests)
  - Bridge starts and stops cleanly with the session
  - Token auth rejects connections with wrong/missing token
  - `just all` passes

### PW.2: Web Invite Link
- **Status**: `[ ]`
- **Depends on**: PW.1, P1.1
- **Description**: When `--web` is enabled, generate a second invite URL alongside the existing `pairwire://` link. The web URL opens in any browser and loads the viewer.
- **Deliverables**:
  - Web invite URL format: `https://join.pairwire.app/s?host=<ip_or_relay>&port=9443&cert=<cert_hash_hex>&token=<session_token>`
    - `host`: the host's public IP (attempt NAT traversal via iroh's relay infrastructure to discover) or relay fallback address
    - `cert`: SHA-256 hash of the self-signed cert (enables `serverCertificateHashes` without a CA)
    - `token`: same session token as the native invite
  - Host terminal output shows both links:
    ```
    Native invite:  pairwire://0f02581240...?relay=...&token=...
    Web invite:     https://join.pairwire.app/s?host=...&cert=...&token=...
    ```
  - QR code for the web invite (using a pure-Rust QR lib — `qrcode` crate) displayed in the menu bar popover
  - `InviteLink` type extended with `Option<WebInvite>` — web invite is absent when `--web` not passed
- **Acceptance criteria**:
  - Web URL parses correctly in JS (`URL` constructor + `searchParams`)
  - `cert` hash matches the self-signed cert served by PW.1
  - QR code renders correctly in terminal (UTF-8 block characters) and menu bar

### PW.3: Web Client — Video Decode + Render
- **Status**: `[ ]`
- **Depends on**: PW.1, PW.2
- **Description**: The browser-side video pipeline. Receives H.264 NAL units over WebTransport, decodes via WebCodecs, renders to a `<canvas>`.
- **Stack**: TypeScript + Vite. No framework — the UI is minimal enough that a framework adds more complexity than it removes. Compiled to a static bundle served from `join.pairwire.app`.
- **Deliverables**:
  - `SessionClient` class: opens WebTransport connection to the bridge using the invite URL params
  - `VideoDecoder` (WebCodecs API): receives encoded chunks from the WebTransport stream, feeds to `VideoDecoder`, paints `VideoFrame` to canvas via `requestVideoFrameCallback`
  - Keyframe detection: if the first chunk is a delta frame, buffer and discard until a keyframe arrives (same rule as native decoder)
  - Canvas: fills the browser viewport, maintains aspect ratio, no scroll bars. Black bars on mismatch (letterbox/pillarbox). `window.addEventListener('resize', ...)` repaints layout.
  - Loading state: "Connecting…" → "Waiting for first frame…" → viewer. Clean error states for connection failure, token rejection, session ended.
  - Browser compatibility: Chrome/Edge (full support), Safari 17+ (WebCodecs H.264 supported), Firefox (WebCodecs behind flag — graceful degradation message)
- **Acceptance criteria**:
  - Video plays in Chrome within 3 seconds of opening the link on a LAN connection
  - Resize browser window → canvas reflows, no distortion
  - Close host → browser shows "Session ended" cleanly (no unhandled promise rejection)

### PW.4: Web Client — Audio Playback
- **Status**: `[ ]`
- **Depends on**: PW.3
- **Description**: Receive and play the host's audio in the browser via Web Audio API.
- **Deliverables**:
  - Receives Opus frames over a dedicated WebTransport stream
  - Decodes via `AudioDecoder` (WebCodecs — supported in Chrome/Edge; polyfill via `libopus` WASM for Safari)
  - Plays via `AudioContext` with a small jitter buffer (~60ms) matching the native decoder
  - Mute button in the viewer UI
  - Autoplay policy handling: Web Audio requires a user gesture before playback. Show a "Click to enable audio" overlay if the page loaded without a user interaction (direct link open). Dismiss on click, start audio.
- **Acceptance criteria**:
  - Host speaks, browser guest hears within ~200ms on LAN
  - Mute/unmute works without reconnecting
  - Autoplay overlay appears when needed, dismisses correctly

### PW.5: Web Client — Cursor, Draw, Reactions
- **Status**: `[ ]`
- **Depends on**: PW.3
- **Description**: The interactive layer for web guests. They can point, draw, and react — they just can't drive.
- **Deliverables**:
  - **Cursor send**: `mousemove` on canvas → normalise to 0.0–1.0 coordinates → `CursorUpdate` datagram → WebTransport. Throttled: only send when moved >2px, idle after 500ms.
  - **Cursor receive**: host's cursor position (already in the captured video — free) and any other web/native guests' cursors rendered as coloured overlays on the canvas. Each web guest gets a colour derived from their session token hash.
  - **Draw**: `mousedown` + `mousemove` + `mouseup` → `DrawPoint` datagrams in real-time + `DrawStroke` on mouse up. Hold `Shift` for laser pointer mode (fades after 2s). Rendered as canvas 2D overlay on top of the video canvas.
  - **Reactions**: floating reaction picker (8 emoji, keyboard shortcuts 1–8) → `Reaction` datagram. Received reactions from all peers animate on the canvas overlay.
  - **No control**: cursor is always `default` on the canvas (no pointer affordance). Clicks are not forwarded as input events. A small badge reads "View only" in the corner.
- **Acceptance criteria**:
  - Web guest moves mouse → native host sees the cursor overlay
  - Web guest sends a reaction → all peers (native + web) see it animate
  - Drawing appears on all peers simultaneously
  - "View only" badge visible and cannot be dismissed

### PW.6: Viewer UI Polish
- **Status**: `[ ]`
- **Depends on**: PW.3, PW.4, PW.5
- **Description**: The web viewer should feel considered, not bolted together. Minimal surface, but everything that's there should be right.
- **Deliverables**:
  - Fullscreen button (`requestFullscreen()` on the canvas container)
  - Connection quality indicator: colour dot (green/yellow/red) based on WebTransport stats (RTT, packet loss via `getStats()`)
  - Guest count badge: "2 others watching" (received via a control datagram from the bridge)
  - Dark background (#0a0a0a) with no chrome — just the screen, the overlays, and a thin control bar that auto-hides after 3s of no mouse movement
  - Page title: "pairwire — live session" while connected, "pairwire — session ended" on disconnect
  - Favicon: small coloured dot (green = connected, grey = disconnected)
  - Mobile: pinch-to-zoom on the canvas (read-only pan/zoom, no input forwarded). Portrait orientation shows a "Rotate for best experience" hint.
- **Acceptance criteria**:
  - Fullscreen works on Chrome, Safari, Edge
  - Control bar hides and reappears correctly
  - Mobile: renders without horizontal scroll, pinch-to-zoom functional

---

### What This Phase Deliberately Excludes

- **No keyboard/mouse injection from browser** — `WebGuestConnected` advertises `capabilities: [Screen, Audio, Cursor, Draw, Reactions]` only. The host will never open an input stream to a web guest.
- **No terminal panel** — out of scope. The native client is the right tool for terminal access.
- **No tunnel access** — web guests cannot request tunnels.
- **No mic input** — receiving host audio is in scope (PW.4). Sending browser mic audio to the host is technically possible but out of scope for v1 (getUserMedia + Web Audio encoding is a project in itself, and voice chat already works natively).
- **No persistent sessions** — the web invite link is valid for the lifetime of the host process. No session replay, no recording access.

### P6.7: Virtual / Headless Display Sharing
- **Status**: `[ ]`
- **Description**: Host shares a virtual display (created via a virtual display driver) that they are not actively looking at. Useful for running a dedicated demo environment, a CI output display, or a secondary workspace that is shared continuously without disrupting the host's primary workflow.
- **Notes**: On macOS, virtual displays can be created with `CGVirtualDisplay` (macOS 12.4+) or via third-party virtual display drivers. The capture pipeline doesn't care — it just gets a `display_id`. The tricky parts are: creating and managing the virtual display lifecycle, routing applications to it (no standard API; user must manually move windows), and handling the case where the virtual display has no physical backing (some APIs behave differently). Lower priority — niche use case but genuinely useful for "always-on" shared workspaces.
- **Deliverables**:
  - `pairwire host --virtual-display` flag: creates a virtual display, shares it, tears it down on exit
  - Virtual display resolution configurable (default: match guest's viewer size from P4.7.1 negotiation)
  - Graceful fallback if virtual display creation fails (log + fall back to primary display)
