# LEARNINGS.md — Quirks, Gotchas, and Discovered Knowledge

> Update this file whenever you discover something unexpected, hit a dead end, or find behavior that contradicts documentation. Future agents (and humans) will thank you.

## Format

Each entry should include:
- **Date** (YYYY-MM-DD)
- **Area** (e.g., iroh, scap, ffmpeg, moq-lite, build)
- **What happened** (brief description)
- **Resolution or workaround** (if any)

---

## Entries

### 2026-03-30 — FFmpeg macOS Homebrew build setup
- **Area**: ffmpeg, build, macOS
- **What**: `cargo build --features pairwire/ffmpeg` fails with two sequential errors on a fresh Mac:
  1. `The system library libavutil was not found` — FFmpeg not installed
  2. `fatal error: '/usr/include/libavcodec/avfft.h' file not found` — Homebrew installs headers under its own prefix, not `/usr/include`
- **Resolution**: Install FFmpeg via Homebrew, then set both `PKG_CONFIG_PATH` and `CPATH` before building:
  ```bash
  brew install ffmpeg
  export PKG_CONFIG_PATH="$(brew --prefix ffmpeg)/lib/pkgconfig:$(brew --prefix)/lib/pkgconfig"
  export CPATH="$(brew --prefix ffmpeg)/include"
  cargo build --workspace --features pairwire/ffmpeg,pairwire/gui
  ```
  Add both exports to `~/.zshrc` to make them permanent. The `CPATH` export is the critical one — without it, `ffmpeg-sys-next`'s bindgen step cannot find the FFmpeg headers even when pkg-config succeeds.

### 2026-03-30 — FFmpeg context must be allocated with codec pointer
- **Area**: ffmpeg-next, capture
- **What**: `codec::context::Context::new()` calls `avcodec_alloc_context3(null)`, which leaves `codec_type = AVMEDIA_TYPE_UNKNOWN`. libx264 (and other encoders) reject this with "Generic error in an external library".
- **Resolution**: Allocate with the codec pointer explicitly:
  ```rust
  let context = unsafe {
      let ptr = avcodec_alloc_context3(codec.as_ptr());
      codec::context::Context::wrap(ptr, None)
  };
  ```
  Then call `.encoder().video()` on the resulting context, and `.open_as_with(codec, opts)` to open.

### 2026-03-30 — OS thread channel design for FFmpeg threads
- **Area**: capture, threading
- **What**: `tokio::sync::mpsc::Receiver::blocking_recv()` requires a tokio runtime on the calling thread. FFmpeg encode/decode threads are plain OS threads with no runtime.
- **Resolution**: Use `std::sync::mpsc` for OS-thread-to-OS-thread channels (capture→encode, encode→decode). Use `tokio::sync::mpsc` only at the async boundary (encode→network sender). `tokio::sync::mpsc::Sender::blocking_send()` is safe from OS threads.

### 2026-03-30 — iroh 0.97 API corrections
- **Area**: iroh, transport
- **What**: Several method names differ from what the docs/examples suggest:
  - `endpoint.id()` not `endpoint.endpoint_id()`
  - `endpoint.addr()` not `endpoint.endpoint_addr()`
  - `endpoint.online().await` to wait for relay assignment (no return value)
  - `conn.remote_id()` not `conn.remote_endpoint_id()`
  - `EndpointId` is a type alias for `PublicKey`; `Display` uses lowercase hex
  - `SecretKey::generate` requires `&mut impl CryptoRng` (not by value)
- **Resolution**: Always check the actual source in `~/.cargo/registry/src/` before writing iroh code.

### 2026-03-30 — rand version conflict with iroh
- **Area**: iroh, transport
- **What**: iroh-base uses `rand_core 0.9`. Using `rand 0.8` in the same crate causes a `CryptoRng` trait mismatch because `rand 0.8` depends on `rand_core 0.6`.
- **Resolution**: Use `rand = "0.9"` in any crate that also depends on iroh. Also: `rand::thread_rng()` is renamed to `rand::rng()` in rand 0.9.

### 2026-03-30 — iroh test compilation is slow
- **Area**: iroh, build
- **What**: `cargo test --workspace` appears to hang because compiling iroh (noq, iroh-relay, etc.) takes 60–90 seconds on first build. It's not actually hung.
- **Resolution**: Run `cargo test -p pairwire-core -p pairwire-protocol -p pairwire-capture -p pairwire-audio` for fast iteration. Run `cargo test -p pairwire-transport` separately with a longer timeout.

### 2026-03-30 — Project Bootstrapped
- **Area**: project
- **What**: Repository initialized with research docs, agent instructions, and phased build plan.
- **Note**: No Cargo workspace exists yet. The scaffold (workspace + crate stubs) is the first task in PLAN.md Phase 0.

### 2026-03-30 — iroh v0.96 hole-punch regression
- **Area**: iroh
- **What**: iroh v0.96 has a known regression where hole-punching is NOT re-triggered when network conditions change after initial connection. If you lose a direct path, you stay on relay. See `docs/01-iroh-noq-connectivity.md`.
- **Resolution**: Pin iroh version carefully. Check if this is fixed in newer releases before upgrading.

### 2026-03-30 — FFmpeg encoder contexts are !Send
- **Area**: ffmpeg
- **What**: `ffmpeg-next` encoder and decoder contexts cannot be sent across threads. They must live on the thread that created them.
- **Resolution**: Use dedicated OS threads for encode/decode. Communicate via channels. Never try to move a codec context into a tokio task.

### 2026-03-30 — scap does not expose dirty rectangles
- **Area**: scap
- **What**: The underlying platform APIs (ScreenCaptureKit, Desktop Duplication, PipeWire) all provide dirty-rectangle metadata, but scap (0.1.0-beta.1) does not expose it.
- **Resolution**: For MVP, encode full frames always. For optimization later, either fork scap to expose dirty rects or use platform APIs directly.

### 2026-03-30 — QUIC datagram max size
- **Area**: transport
- **What**: QUIC datagrams are limited to path MTU minus overhead, typically ~1200 bytes. No fragmentation — if your payload exceeds this, it's your problem.
- **Resolution**: Keep datagram payloads small. Cursor updates (~50 bytes) and heartbeats are fine. Anything larger must use streams.

### 2026-03-30 — postcard is not self-describing
- **Area**: protocol
- **What**: `postcard` serialization format does not include type information. Both sides must have exactly the same type definition to deserialize correctly.
- **Resolution**: Version all protocol types. Include a version field in the session handshake. Never change existing field order — only append new fields.
