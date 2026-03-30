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
- **Status**: `[ ]`
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
- **Status**: `[ ]`
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
- **Status**: `[ ]`
- **Depends on**: P1.2
- **Description**: Send and receive cursor position updates via QUIC datagrams.
- **Deliverables**:
  - `CursorSender` — polls cursor position, sends `CursorUpdate` datagrams at 60Hz when moving, throttles when still
  - `CursorReceiver` — receives datagrams, exposes latest cursor position via `Arc<AtomicCell<CursorPosition>>`
  - Platform cursor position reading (macOS: `CGEvent`)
- **Acceptance criteria**:
  - Test: send 100 cursor updates, verify latest position is accessible (not all — datagrams are lossy)
  - Verify `CursorUpdate` serialized size < 50 bytes
  - `just all` passes
- **Docs**: `docs/03-quic-transport-primitives.md` (datagram section)

### P1.9: Basic Viewer UI
- **Status**: `[ ]`
- **Depends on**: P1.5, P1.8
- **Description**: Minimal UI window that displays the decoded remote screen and overlays the remote cursor.
- **Deliverables**:
  - egui (or iced) window that displays a texture from `DecodedFrame`
  - GPU texture upload (wgpu) — update texture each frame from the decode channel
  - Remote cursor overlay (colored dot/arrow at cursor position)
  - FPS counter and connection status display
  - Window title shows peer EndpointId (truncated)
- **Acceptance criteria**:
  - Window opens, displays frames from `FakeFrameSource` (no network needed)
  - Cursor overlay renders at correct position
  - FPS counter works
  - `just all` passes

### P1.10: End-to-End Integration — Two Peers
- **Status**: `[ ]`
- **Depends on**: P1.1 through P1.9
- **Description**: Wire everything together. Two instances of the app connect via iroh, one shares screen + cursor, the other views.
- **Deliverables**:
  - CLI interface: `rust-sharing host` (captures and shares) / `rust-sharing join <invite>` (views)
  - Host: capture → encode → send + cursor send
  - Guest: receive → decode → display + cursor receive
  - Invite link generation and display on host startup
  - Graceful shutdown (Ctrl+C)
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
- **Status**: `[ ]`
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
- **Status**: `[ ]`
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
- **Status**: `[ ]`
- **Depends on**: P2.2
- **Description**: Implement remote control in `crates/input`. Guest sends keyboard and mouse events; host injects them into the OS.
- **Deliverables**:
  - `InputSender` — captures local keyboard/mouse events, serializes, sends on input stream
  - `InputReceiver` — receives events, injects into OS via platform APIs
  - macOS injection: `CGEvent` posting
  - Permission model: host must explicitly grant control
  - `ControlHandoff` message for granting/revoking permissions
- **Acceptance criteria**:
  - Test: serialize and deserialize all input event types
  - Manual test: guest types, host sees keystrokes
  - Permission check: events are rejected when control not granted
  - `just all` passes
- **Docs**: `docs/05-concrete-architecture.md` (remote control protocol section)

### P2.4: Clipboard Sync
- **Status**: `[ ]`
- **Depends on**: P2.2
- **Description**: Bidirectional clipboard synchronization between peers.
- **Deliverables**:
  - Clipboard monitoring (detect changes)
  - `ClipboardSync` message with format detection (text, image, files)
  - Configurable: auto-sync or manual
  - Size limit to prevent accidental large transfers
- **Acceptance criteria**:
  - Test: text clipboard round-trips between peers
  - Large clipboard data (>1MB) is rejected or chunked
  - `just all` passes

### P2.5: Quality Adaptation
- **Status**: `[ ]`
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
- **Acceptance criteria**:
  - Test with simulated degradation: verify encoder config changes
  - Heartbeat messages round-trip correctly
  - `just all` passes
- **Docs**: `docs/05-concrete-architecture.md` (degradation handling)

### P2.6: Skip Static Frames Optimization
- **Status**: `[ ]`
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
- **Status**: `[ ]`
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
- **Status**: `[ ]`
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
- **Status**: `[ ]`
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
- **Status**: `[ ]`
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
- **Status**: `[ ]`
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
- **Status**: `[ ]`
- **Depends on**: P1.9
- **Description**: Minimal system tray (macOS menu bar) with session controls.
- **Deliverables**:
  - Start/stop sharing
  - Copy invite link
  - Connection status indicator
  - Disconnect button
  - Preferences shortcut
- **Acceptance criteria**:
  - macOS: menu bar extra appears and functions
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
- **Description**: Record screen + audio to a local file during a session.

### P5.2: Annotation / Drawing Tools
- **Status**: `[ ]`
- **Description**: Draw on the shared screen. Strokes via datagrams (points) + reliable stream (stroke completion).

### P5.3: Multi-Party Sessions (3+ Peers)
- **Status**: `[ ]`
- **Description**: Extend protocol for N peers with a relay/mesh topology.

### P5.4: AI-Assisted Pairing
- **Status**: `[ ]`
- **Description**: Shared LLM context in the pairing session. "Ask the AI about this code" as a first-class feature.

### P5.5: Editor Integration (LSP-Aware Cursors)
- **Status**: `[ ]`
- **Description**: Share editor state at the semantic level (LSP events, cursor position in file) alongside screen sharing.
