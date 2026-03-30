# Phase 0: Project Foundation

> Goal: Green workspace that compiles, lints, tests, and gives agents a working baseline.

## Tasks

### P0.1 — Initialize Cargo Workspace
- **Status**: ✅ Complete
- **Commit**: (see git log)
- **Crates created**:
  - `crates/core` — shared types, config, errors
  - `crates/protocol` — wire protocol messages
  - `crates/transport` — iroh + QUIC helpers (stub)
  - `crates/capture` — screen capture + encode (stub)
  - `crates/audio` — audio capture + Opus (stub)
  - `crates/input` — keyboard/mouse/clipboard (stub)
  - `crates/terminal` — PTY sharing (stub)
  - `crates/app` — binary entry point
- **Notes**: Workspace uses resolver = "2", edition 2024.

### P0.2 — Core Error and Config Types
- **Status**: ✅ Complete
- **Deliverables**:
  - `Error` enum (thiserror): Transport, Capture, Encode, Decode, Audio, Protocol, Io, Serialization, ConnectionClosed, PermissionDenied, VersionMismatch
  - `Result<T>` type alias, re-exported from crate root
  - `SessionConfig`, `CaptureConfig`, `AudioConfig` with serde derives
  - `VideoEncoder` enum (H264VideoToolbox, H264Software)
  - `PeerId` (32-byte ed25519 key wrapper), `SessionId` (16-byte)
- **Notes**: `core` has no tokio dependency — kept lean intentionally.

### P0.3 — Protocol Message Types
- **Status**: ✅ Complete
- **Deliverables**:
  - `control.rs`: `Handshake`, `ControlMessage`, `ControlHandoff`, `Heartbeat`, `Capability`, `Permissions`, `VideoCodec`, `AudioCodec`
  - `cursor.rs`: `CursorUpdate` (< 50 bytes verified by test)
  - `input.rs`: `KeyEvent`, `MouseClick`, `MouseScroll`, `ClipboardSync`, `InputMessage` envelope
  - `media.rs`: `ScreenFrameHeader`, `FrameFlags`
  - `PROTOCOL_VERSION = 1` constant
- **Tests**: All types have round-trip tests. Size assertions on `CursorUpdate` and `Heartbeat`.

### P0.4 — CONVENTIONS.md
- **Status**: ✅ Complete
- **Deliverables**:
  - `CONVENTIONS.md` at repo root
  - Sections: Error Handling, Async & Threading, Channel Patterns, Serialization, Logging, Testing, Naming, Unsafe, Feature Flags
  - CLAUDE.md updated to reference CONVENTIONS.md as "ground truth"

### P0.5 — Test Harness & Stubs
- **Status**: ✅ Complete
- **Deliverables**:
  - `crates/core/src/test_utils.rs`: `FakeFrameSource`, `FakeAudioSource`, `LoopbackChannel`
  - `crates/protocol/src/test_utils.rs`: `assert_round_trip<T>`, factory functions for all message types
  - `tests/fixtures/README.md`: placeholder for future binary fixtures
- **Notes**: `LoopbackChannel` uses `std::sync::mpsc` (not tokio) to keep `core` dependency-free.

## Validation

```
cargo check --workspace   ✅
cargo clippy -- -D warnings  ✅
cargo test --workspace    ✅  (23 tests passing)
cargo fmt -- --check      ✅
```
