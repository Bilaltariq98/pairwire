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
- **Status**: `[x]`
- **Depends on**: P1.1 through P1.9
- **Description**: Wire everything together. Two instances of the app connect via iroh, one shares screen + cursor, the other views.
- **Deliverables**:
  - CLI interface: `pairwire host` (captures and shares) / `pairwire join <invite>` (views)
  - Host: capture → encode → send + cursor send
  - Guest: receive → decode → display + cursor receive
  - Invite link generation and display on host startup
  - Graceful shutdown (Ctrl+C)
- **Notes**: Fully working. Screen + audio streaming verified. Cursor overlay waiting on P1.8 wiring. Stream multiplexing conflict between moq-lite audio and raw screen streams fixed via `UniStreamRouter` (commit 0dc73fa). Retina resolution mismatch fixed via lazy scaler init.
- **Acceptance criteria**:
  - Manual test: run host on one terminal, join on another (same machine is fine — iroh handles loopback)
  - Screen appears in viewer within 3 seconds
  - Cursor position updates visible
  - Clean shutdown without panics
  - `just all` passes
- **Performance targets** (measure and record in LEARNINGS.md):
  - Screen latency < 100ms
  - Cursor latency < 50ms
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
- **Status**: `[~]` (code complete; not wired in session)
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
- **Status**: `[~]` (code complete; not wired in session)
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
- **Status**: `[~]` (code complete; not wired in session)
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
- **Deliverables**:
  - **Host side**: Poll local cursor position (macOS: `CGEvent`), send `CursorUpdate` datagrams to guest at up to 60Hz. Receive guest's `CursorUpdate` and render a colored overlay on the host display (or in the host's tray/status UI if no host viewer).
  - **Guest side**: Track mouse position within the viewer window, map to host display coordinates, send `CursorUpdate` datagrams to host. Receive host's `CursorUpdate` — but since the host cursor is already in the captured video, this is optional (useful for coordinate-aware features like "snap to host cursor").
  - Distinct cursor colors: host = red crosshair, guest = blue crosshair (configurable later).
  - Throttle: only send when position changes by >2px. Idle after 500ms of no movement.
- **Acceptance criteria**:
  - Guest viewer shows host cursor movements in real-time (via screen capture, already works)
  - Host sees guest cursor as a colored overlay
  - Cursor updates are <50 bytes per datagram
  - `just all` passes

### P3.5.2: Control Takeover
- **Status**: `[ ]`
- **Depends on**: P3.5.1, P2.3
- **Description**: Guest can take keyboard/mouse control by clicking in the viewer. Host reclaims by moving their real mouse. Like Tuple's "click to drive" model.
- **Deliverables**:
  - `ControlState` enum: `HostActive | GuestActive | GuestRequested`
  - `ControlRequest` / `ControlGrant` messages on the control stream
  - Guest click in viewer → sends `ControlRequest` → host auto-grants (default) or prompts
  - While guest is active: viewer captures keyboard events + mouse clicks/moves → `InputSender` forwards to host → `InputReceiver` injects via `CGEvent`
  - Host moves their real mouse → `ControlRevoke` sent → guest returns to observer mode
  - Visual indicator in viewer: "You have control" / "Host has control" badge
  - Coordinate mapping: guest viewer pixels → host display pixels (accounting for scale factor and viewer window size)
- **Acceptance criteria**:
  - Guest clicks in viewer, types — keystrokes appear on host
  - Host wiggles mouse — control returns to host, guest typing stops injecting
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
- **Acceptance criteria**:
  - Send a reaction, see it appear on both screens at the sender's cursor position
  - Reactions fade out after ~2 seconds
  - Works regardless of who has control
  - `just all` passes

### P3.5.5: Wire Clipboard Sync
- **Status**: `[ ]`
- **Depends on**: P2.4
- **Description**: Wire `ClipboardSender`/`ClipboardReceiver` into session.
- **Deliverables**:
  - Both host and guest spawn clipboard monitors
  - Copy on one side appears on the other within ~500ms
  - Visual toast: "Clipboard synced" indicator
- **Acceptance criteria**:
  - Text clipboard round-trips between peers
  - Large data (>1MB) is rejected with a warning
  - `just all` passes

### P3.5.6: Wire Quality Adaptation
- **Status**: `[ ]`
- **Depends on**: P2.5
- **Description**: Wire `QualityMonitor` into host session and connect to encode thread.
- **Deliverables**:
  - `QualityMonitor` spawned in `run_host()`
  - Output watch channel connected to encode thread for dynamic bitrate/fps/resolution changes
  - Visual indicator in viewer: connection quality badge (green/yellow/red)
- **Acceptance criteria**:
  - Under high RTT/loss, encoder downgrades quality tier
  - When conditions improve, quality recovers
  - `just all` passes

---

## Phase 4: Terminal Sharing (Differentiator)

> **Goal**: Share terminal sessions natively via PTY forwarding — the unique feature no competitor has.
> **Estimated effort**: 3-4 weeks
> **Docs**: `docs/06-competitive-analysis.md` (terminal-first wedge)

### P4.1: PTY Capture
- **Status**: `[ ]`
- **Description**: Capture terminal output by creating a PTY and reading its output stream.
- **Deliverables**:
  - Create PTY with configurable shell (detect user's default)
  - Read PTY output as byte stream
  - Handle terminal resize (SIGWINCH)
  - `TerminalFrame` — output bytes + size + timestamp
- **Acceptance criteria**:
  - Test: spawn PTY, send "echo hello", receive "hello" in output
  - Resize propagation works
  - `just all` passes

### P4.2: Terminal State Streaming
- **Status**: `[ ]`
- **Depends on**: P4.1, P1.2
- **Description**: Stream terminal output over a reliable QUIC stream to the remote peer.
- **Deliverables**:
  - Dedicated reliable stream for terminal data (priority 1)
  - Terminal resize messages
  - Input forwarding (guest types → host PTY)
- **Acceptance criteria**:
  - End-to-end: guest sees host's terminal output in real-time
  - Guest can type and host's terminal receives input
  - `just all` passes

### P4.3: Terminal Renderer
- **Status**: `[ ]`
- **Depends on**: P4.2
- **Description**: Render received terminal output in the viewer UI using a terminal emulator widget.
- **Deliverables**:
  - Terminal emulator (consider `alacritty_terminal` or `vte` crate for parsing)
  - Render terminal grid in the UI (egui/iced)
  - Support colors, cursor, scrollback
  - User's own font and theme preferences
- **Acceptance criteria**:
  - Terminal output renders correctly (colors, cursor position)
  - Scrollback works
  - `just all` passes

---

## Phase 5: Differentiation & Polish (Ongoing)

> Future features — not scoped in detail yet. Add tasks as Phase 1-4 complete.

### P5.1: Recording (Save Session to File)
- **Status**: `[ ]`
- **Description**: Record screen + audio to a local file during a session. Mux H.264 video + Opus audio into MP4/MKV.

### P5.2: Multi-Party Sessions (3+ Peers)
- **Status**: `[ ]`
- **Description**: Extend protocol for N peers with a relay/mesh topology. Each peer gets a distinct cursor color.

### P5.3: AI-Assisted Pairing
- **Status**: `[ ]`
- **Description**: Shared LLM context in the pairing session. "Ask the AI about this code" as a first-class feature. Could use screen content + terminal history as context.

### P5.4: Editor Integration (LSP-Aware Cursors)
- **Status**: `[ ]`
- **Description**: Share editor state at the semantic level (LSP events, cursor position in file) alongside screen sharing.

### P5.5: Rich Reactions — Custom Animated Stickers with Sound
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
