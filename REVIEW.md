# Code Review — Architecture & Technical Findings

> Generated 2026-04-02. Reference this file when tackling fixes.
> Each finding has a severity, location, description, and suggested fix.

---

## Status Key

- `[ ]` — Not started
- `[~]` — In progress
- `[x]` — Fixed

---

## Critical

### R-C1: Frame Hash Only Samples 1/64th of Data
- **Status**: `[x]`
- **File**: `crates/capture/src/encode_thread.rs:293-299`
- **Issue**: `hash_frame()` only hashes byte 0 of every 64-byte chunk. 98.4% of frame data is ignored. Nearly-identical frames hash the same, causing the encoder to skip real screen changes as "static frames."
- **Fix**: Hash the full buffer: `data.hash(&mut h)` instead of sampling.

### R-C2: Unchecked Bounds in `fill_bgra_frame`
- **Status**: `[x]`
- **File**: `crates/capture/src/encode_thread.rs:283-291`
- **Issue**: No validation that FFmpeg's frame stride >= row width, or that input data length matches `width * height * 4`. Panics or memory corruption on malformed frames.
- **Fix**: Validate `data.len() >= width * height * 4`, `stride >= row_bytes`, and `row * stride + row_bytes <= dst.len()` before copying. Return `Result` instead of `()`.

### R-C3: `unwrap()` on Scaler Option
- **Status**: `[x]`
- **Files**: `crates/capture/src/encode_thread.rs:186`, `crates/capture/src/decode_thread.rs:170`
- **Issue**: Direct `.unwrap()` outside tests on the scaler `Option`. If initialization fails or logic changes, the encode/decode thread panics instead of propagating an error.
- **Fix**: Replace with `.ok_or_else(|| Error::Encode("scaler not initialized".into()))?`.

### R-C4: Missing Peer Authentication
- **Status**: `[ ]`
- **Files**: `crates/transport/src/transport.rs:24-106`, `crates/transport/src/handshake.rs`
- **Issue**: No verification that the connecting peer's endpoint ID matches the one in the invite link. A malicious peer can connect to any session.
- **Fix**: In the handshake, verify `conn.remote_id()` matches the expected endpoint ID from the invite link. Reject mismatches.

### R-C5: String-Based Error Classification
- **Status**: `[x]`
- **File**: `crates/transport/src/connection.rs:69-80, 104-116`
- **Issue**: Uses `msg.contains("closed")` / `msg.contains("reset")` to classify connection errors. Iroh could change message wording between versions, silently breaking error handling.
- **Fix**: Inspect actual error enum variants from `iroh::endpoint` instead of string matching. Add a fallback for unknown variants.

---

## High

### R-H1: Task Abort Without Graceful Shutdown
- **Status**: `[x]`
- **File**: `crates/transport/src/moq_session.rs` (feed task abort paths)
- **Issue**: Feed tasks are `.abort()`-ed on stop, which can corrupt moq-lite producer state (incomplete groups sent to peer).
- **Fix**: Pass a `oneshot::Receiver` to feed tasks. On shutdown signal, finish the current group, close the producer, then exit.

### R-H2: Mutex Deadlock Risk in Stream Router
- **Status**: `[x]` (file deleted in P2.1.1)
- **File**: `crates/transport/src/stream_router.rs:69-71, 96-98`
- **Issue**: `NonScreenReceiver.rx` is behind `Arc<tokio::sync::Mutex>`. If a consumer panics while holding the lock, the entire moq-lite session hangs permanently.
- **Fix**: Replace with a dedicated receiver task that forwards, or use `tokio::sync::broadcast`. Remove the Mutex from the hot path.

### R-H3: Unbounded Vec Allocation from Network
- **Status**: `[ ]`
- **File**: `crates/transport/src/connection.rs:248`
- **Issue**: `recv_message` allocates `vec![0u8; len]` up to `MAX_MESSAGE_LEN` (~4MB) per message with no per-connection quota. Multiple concurrent reads from a malicious peer can cause OOM.
- **Fix**: Add a per-connection concurrent allocation limit. Consider a bounded buffer pool.

### R-H4: Audio Resource Leak on Error
- **Status**: `[ ]`
- **File**: `crates/app/src/session.rs:247-251`
- **Issue**: If `run_host()`/`run_join()` returns early or panics after audio threads are spawned, they run forever. No `Drop` impl or RAII guard.
- **Fix**: Implement `Drop` on audio handle structs to signal stop automatically, or use a scoped guard (e.g., `scopeguard::defer!`).

### R-H5: Keepalive/Empty Packet Ambiguity
- **Status**: `[ ]`
- **Files**: `crates/capture/src/encode_thread.rs:162-169`, `crates/capture/src/decode_thread.rs:135`
- **Issue**: Keepalive packets have `data: Bytes::new()`. Decoder has no validation that non-keepalive packets contain data. An empty encoder output packet silently passes through and can crash FFmpeg.
- **Fix**: Either create a separate `KeepAlivePacket` type, or add `if pkt.is_keepalive { continue; }` guard in decode loop and treat null `pkt.data()` from encoder as an error.

### R-H6: `chunks_exact(4)` Panic on Malformed Data
- **Status**: `[x]`
- **File**: `crates/capture/src/capture_thread.rs:240`
- **Issue**: BGR0 frame conversion panics if `data.len() % 4 != 0`.
- **Fix**: Validate `data.len() % 4 == 0` before the loop. Return `None` with a debug log on failure.

### R-H7: Workspace Dependency Inconsistency
- **Status**: `[ ]`
- **Files**: `crates/transport/Cargo.toml:19-20`, `crates/app/Cargo.toml:15-16`, `crates/input/Cargo.toml:10`
- **Issue**: `pairwire-capture`, `pairwire-audio`, `pairwire-transport` referenced via `path = "../..."` instead of `workspace = true`. `hex` is duplicated across crates without workspace declaration.
- **Fix**: Move `hex = "0.4"` to `[workspace.dependencies]`. Update all path-based internal crate references to use `workspace = true`.

### R-H8: `capture` Unconditionally Enables `test-support`
- **Status**: `[ ]`
- **File**: `crates/capture/Cargo.toml:8`
- **Issue**: `pairwire-core` is pulled with `features = ["test-support"]` in regular deps. Production binaries compile test utilities.
- **Fix**: Move `pairwire-core = { ..., features = ["test-support"] }` to `[dev-dependencies]`. Use plain `pairwire-core.workspace = true` in `[dependencies]`.

---

## Medium

### R-M1: Config Allows Invalid States
- **Status**: `[ ]`
- **File**: `crates/core/src/config.rs`
- **Issue**: `fps: 0`, `width: 0`, `channels: 0`, `frame_duration_ms: 7` (not valid for Opus) are all representable. No validation at construction. Causes division-by-zero or encoder misconfiguration.
- **Fix**: Add a `validate()` method (or use `TryFrom` newtypes) that rejects: `fps == 0`, `width == 0`, `height == 0`, `channels not in [1, 2]`, `frame_duration_ms not in [5, 10, 20, 40, 60]`, `sample_rate not in [8000, 12000, 16000, 24000, 48000]`. Call it at session startup.

### R-M2: Error Enum Too Generic
- **Status**: `[ ]`
- **File**: `crates/core/src/error.rs:7-41`
- **Issue**: `Transport(String)`, `Encode(String)`, `Protocol(String)` — all string-wrapped. No structured variants for timeout, congestion, version mismatch, or corruption. Makes retry logic and diagnostics impossible.
- **Fix**: Expand with structured variants: `Timeout`, `ConnectionReset`, `VersionMismatch { local, remote }`, `UnsupportedCodec(String)`, `Corrupted`. Use `#[from]` for `postcard::Error` and `std::io::Error`.

### R-M3: Handshake Accepts Unbounded Vectors
- **Status**: `[x]`
- **File**: `crates/protocol/src/control.rs:5-23`
- **Issue**: `capabilities`, `video_codecs`, `audio_codecs` are `Vec<_>` with no max length. Malicious peer can exhaust memory.
- **Fix**: After deserialization, validate: `capabilities.len() <= 16`, `video_codecs.len() <= 8`, `audio_codecs.len() <= 4`. Reject with `Error::Protocol` on overflow.

### R-M4: No Backpressure in moq Screen/Audio
- **Status**: `[x]` (files deleted in P2.1.1; hang OrderedConsumer handles latency)
- **Files**: `crates/transport/src/moq_screen.rs`, `crates/transport/src/moq_audio.rs`
- **Issue**: Frames pile up in moq-lite internal buffers during congestion with no flow control or depth checking. Causes unbounded latency spikes.
- **Fix**: Check buffer depth before appending. On backlog, drop frames and force a keyframe resync.

### R-M5: Permission Check Race in Input
- **Status**: `[x]` (uses AtomicU8 — no race)
- **File**: `crates/input/src/receiver.rs:105`
- **Issue**: Permission is loaded once per event loop iteration. A stale value can allow one injected event after permission is revoked.
- **Fix**: Check permission immediately before injection, not at loop entry.

### R-M6: Silence Skipping Creates Timestamp Gaps
- **Status**: `[ ]`
- **File**: `crates/audio/src/encode.rs:134-138`
- **Issue**: Skipped silence frames increment the timestamp but send nothing. Decoder/jitter buffer sees discontinuity, causing audio stutters.
- **Fix**: Send an explicit silence marker packet (empty data, valid timestamp + duration) instead of skipping entirely.

### R-M7: Incomplete Handshake Validation
- **Status**: `[x]`
- **File**: `crates/transport/src/handshake.rs:84-91, 113-120`
- **Issue**: No check for `display_width > 0`, `display_height > 0`, or non-empty codec lists. Zero dimensions cause downstream division-by-zero.
- **Fix**: Add validation in `negotiate()`: reject if dimensions are 0 or codec lists are empty.

### R-M8: No Timeout on Stream Accept Loops
- **Status**: `[x]` (file deleted in P2.1.1)
- **File**: `crates/transport/src/stream_router.rs:40-65`
- **Issue**: `accept_uni()` / `accept_bi()` block forever if a peer opens a stream but never sends data.
- **Fix**: Wrap accept loops with `tokio::time::timeout()`. Close connection if no activity for N seconds.

### R-M9: Config Round-Trip Test Only Checks 2 Fields
- **Status**: `[x]`
- **File**: `crates/core/src/config.rs:94-100`
- **Issue**: Test asserts `fps` and `sample_rate` but not full struct equality. Field additions escape coverage.
- **Fix**: Derive `PartialEq` on all config types, then `assert_eq!(decoded, cfg)`.

### R-M10: Silent Frame Drops Without Backpressure Signal
- **Status**: `[x]` (files deleted in P2.1.1)
- **Files**: `crates/transport/src/screen.rs:280-283`, `crates/transport/src/moq_audio.rs:226-230`
- **Issue**: All receive paths use `try_send()` which silently drops frames if the channel is full. No signal to sender to slow down or resync. Audio/video can permanently desynchronize.
- **Fix**: Implement adaptive frame skipping (drop every Nth frame) or a shared backpressure token between audio and video channels.

### R-M11: Inconsistent Error Type Usage in Transport
- **Status**: `[x]` (files deleted in P2.1.1)
- **Files**: `crates/transport/src/moq_screen.rs`, `moq_audio.rs`
- **Issue**: Error wrapping uses generic `.map_err(|e| Error::Transport(format!(...)))`. Real errors (congestion, timeouts) are buried in strings and cannot be pattern-matched or retried.
- **Fix**: Define structured `TransportError` variants and implement `From<moq_lite::Error>`.

### R-M12: Mutex `unwrap()` in Audio Playback
- **Status**: `[x]`
- **File**: `crates/audio/src/playback.rs:107, 137`
- **Issue**: `jitter_read.lock().unwrap()` / `jitter_write.lock().unwrap()` — mutex poisoning panics crash the playback thread.
- **Fix**: Use `.unwrap_or_else(|e| e.into_inner())` to recover from poisoned locks.

### R-M13: `ScreenFrameHeader` Comment Misleads About Layout
- **Status**: `[x]` (file deleted in P2.1.1)
- **File**: `crates/protocol/src/media.rs:7-13`
- **Issue**: Doc comment describes a raw 8-byte binary layout, but the struct uses postcard serialization which doesn't guarantee that layout. Misleading for future contributors.
- **Fix**: Update the comment to clarify it uses postcard, not raw bytes. Remove the byte layout description or label it as aspirational.

---

## Low

### R-L1: Magic Numbers in Quality Thresholds
- **Status**: `[x]`
- **File**: `crates/transport/src/quality.rs:74, 76`
- **Issue**: Thresholds (15.0, 5.0, 200) are hardcoded inline. Makes tuning difficult.
- **Fix**: Extract to named `const` values at module level with comments.

### R-L2: No Frame Drop Counters/Metrics
- **Status**: `[ ]`
- **File**: `crates/capture/src/capture_thread.rs:94-96`
- **Issue**: `try_send` failures are logged at `debug` level only. No counter or metric. Can't determine if channel capacity is appropriate.
- **Fix**: Add `AtomicU64` counter for dropped frames. Expose via a diagnostics API or periodic `info!` log.

### R-L3: `SystemTime::now()` Failure Silently Returns 0
- **Status**: `[x]`
- **File**: `crates/capture/src/capture_thread.rs:216-219`
- **Issue**: `unwrap_or_default()` on `duration_since(UNIX_EPOCH)` silently returns 0 if the system clock is invalid.
- **Fix**: Log a warning when the fallback triggers.

### R-L4: `FrameFlags` Uses Two Bools Instead of Bitfield
- **Status**: `[x]` (file deleted in P2.1.1)
- **File**: `crates/protocol/src/media.rs:23-29`
- **Issue**: `FrameFlags` is a struct with two `bool` fields (2 bytes). A `u8` bitfield would be 1 byte — saves 1 byte per frame header over the wire.
- **Fix**: Replace with `FrameFlags(u8)` and accessor methods (`keyframe()`, `last_in_group()`).

### R-L5: Test Coverage Gaps — Protocol Types
- **Status**: `[ ]`
- **Files**: `crates/protocol/src/test_utils.rs`, `crates/transport/src/wt_session.rs`, `crates/transport/src/stream_router.rs`
- **Issue**: No adversarial/boundary tests for protocol types (MAX values, empty collections, truncated input). No tests for stream router tagging or wt_session prefix handling.
- **Fix**: Add boundary tests with MAX integer values, empty vectors, truncated payloads. Add unit tests for stream router and prefix byte handling.

### R-L6: `KeyEvent::modifiers` Bit Layout Not Documented as Constants
- **Status**: `[x]`
- **File**: `crates/protocol/src/input.rs:13`
- **Issue**: Comment says "bit 0=Shift, 1=Ctrl, 2=Alt, 3=Meta/Cmd" but no named constants.
- **Fix**: Define `pub const MOD_SHIFT: u8 = 0x01; MOD_CTRL: u8 = 0x02; MOD_ALT: u8 = 0x04; MOD_META: u8 = 0x08;`.

### R-L7: Incomplete Logging in Screen Transport
- **Status**: `[x]` (file deleted in P2.1.1)
- **File**: `crates/transport/src/screen.rs:81`
- **Issue**: Only logs on keyframe, not delta frames. Hard to debug stream lifecycle.
- **Fix**: Add periodic frame count to `debug!` logs (e.g., every 100 frames).

### R-L8: `tokio` Uses `features = ["full"]`
- **Status**: `[ ]`
- **File**: `Cargo.toml:26` (workspace)
- **Issue**: All crates get all tokio features. Increases compile time and binary size.
- **Fix**: Enumerate only needed features in workspace (`sync`, `rt-multi-thread`, `macros`, `time`, `io-util`). Let individual crates add extras if needed.

---

## Architectural Recommendations

These are not bugs but structural improvements to tackle when the codebase grows.

### R-A1: Decompose Transport Crate
- **Current**: 3000+ lines, 12 modules mixing low-level QUIC with high-level media session logic.
- **Target**: Split into `transport-core` (iroh, QUIC, stream routing), `media` (moq screen + audio), `cursor`, `quality`, `invite` (pure encoding, no network deps).

### R-A2: Decouple Input from Transport
- **Current**: `pairwire-input` depends directly on `pairwire-transport`.
- **Target**: Define an `InputTransport` trait in `core`. Have `input` depend on the trait, `transport` implement it.

### R-A3: Split Session Orchestrator
- **Current**: `app/session.rs` (381 lines) owns all pipelines — a god object.
- **Target**: Extract `AudioSession` and `VideoSession` types, each owning their capture-encode-send pipeline and cleanup.

### R-A4: Standardize Shutdown Model
- **Current**: Mix of `AtomicBool` flags, `.abort()`, `oneshot` channels for shutdown.
- **Target**: Use `tokio::sync::broadcast` (or `CancellationToken`) for coordinated shutdown across all tasks and threads.

### R-A5: Structured Error Architecture
- **Current**: String-wrapped errors everywhere lose context.
- **Target**: Structured `TransportError`, `CodecError`, `ProtocolError` sub-enums with `From` impls. Enables retry logic, metrics, and user-facing diagnostics.
