# LEARNINGS.md

## cpal 0.17: SampleRate is a type alias, not a tuple struct

`cpal::SampleRate` was a newtype tuple struct in older versions but is now a plain type alias for `u32` in 0.17. Constructing it with `cpal::SampleRate(value)` fails with E0423. Use the raw `u32` directly in `StreamConfig`:

```rust
// Wrong (pre-0.17):
sample_rate: cpal::SampleRate(config.sample_rate),

// Correct (0.17+):
sample_rate: config.sample_rate,
```

Also: `DeviceTrait::name()` is deprecated in 0.17 — use `device.description()` instead.
`description()` returns `Result<DeviceDescription, _>`, not `Result<String, _>`. `DeviceDescription` does not implement `Default`, so `.unwrap_or_default()` won't compile. Use `.map(|d| d.name).unwrap_or_default()` to get the name string. — Quirks, Gotchas, and Discovered Knowledge

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
- **Resolution**: Install FFmpeg via Homebrew, set env vars, and use `ffmpeg-next = "8"` (not 6 or 7):
  ```bash
  brew install ffmpeg
  export PKG_CONFIG_PATH="$(brew --prefix ffmpeg)/lib/pkgconfig:$(brew --prefix)/lib/pkgconfig"
  export BINDGEN_EXTRA_CLANG_ARGS="-I$(brew --prefix ffmpeg)/include"
  cargo build --workspace --features pairwire/ffmpeg,pairwire/gui
  ```
  Add both exports to `~/.zshrc` to make them permanent.
- **Version mapping** (critical): Homebrew ships FFmpeg 7.x. `avfft.h` was removed in FFmpeg 7. The crate version must match:
  - `ffmpeg-next = "6"` → targets FFmpeg 6.x → `avfft.h` missing error on Homebrew
  - `ffmpeg-next = "7"` → targets FFmpeg 6.x headers → same error
  - `ffmpeg-next = "8"` → targets FFmpeg 7.x → ✅ works with Homebrew FFmpeg
- `CPATH` does NOT work — bindgen invokes clang directly and ignores it. Use `BINDGEN_EXTRA_CLANG_ARGS`.

### 2026-03-30 — web-transport-iroh is pinned to iroh 0.96, not 0.97
- **Area**: moq-lite, iroh, web-transport
- **What**: `web-transport-iroh 0.2.2` requires `iroh ^0.96.1`. We use iroh 0.97. The `Connection` types are from different crate versions and are incompatible — `WtSession::raw(conn)` fails with a type mismatch.
- **Resolution**: Implement `web_transport_trait::Session` directly on our `PairwireConnection` in `crates/transport/src/wt_session.rs`. The trait is small (~6 async methods) and iroh 0.97's stream types satisfy all of them. Drop `web-transport-iroh` from deps entirely; add `web-transport-trait = "0.3"` directly.
- **Note**: If `web-transport-iroh` releases a 0.3.x targeting iroh 0.97+, we can switch back and delete `wt_session.rs`.

### 2026-03-30 — moq-lite OriginProducer/BroadcastProducer API
- **Area**: moq-lite 0.15
- **What**: The API is not what the docs imply. Key correct patterns:
  - `BroadcastProducer::create_track(Track { name, priority })` — not `insert_track`
  - `origin.publish_broadcast(path, broadcast.consume())` — not `origin.publish(...)`
  - `BroadcastConsumer::subscribe_track(&Track { .. })` — direct, no async wait
  - `origin.consume().announced().await` — returns `Option<(path, Option<BroadcastConsumer>)>`
  - `TrackProducer::append_group()` / `GroupProducer::write_frame(bytes)` / `GroupProducer::finish()`
  - `TrackConsumer::next_group()` / `GroupConsumer::next_frame()` / `FrameConsumer::read_all()`
  - `TrackProducer::latest()` is on `TrackConsumer`, not `TrackProducer`

### 2026-03-30 — iroh 0.97 PathId is private; use PathWatcher for RTT
- **Area**: iroh, quality adaptation
- **What**: `conn.rtt(PathId)` requires constructing `PathId`, which is a private tuple struct in `noq-proto`. Cannot be used from outside the crate.
- **Resolution**: Use `conn.paths().get()` which returns `PathInfoList`. Iterate with `list.iter()` (yields `&PathInfo`), then call `path_info.rtt()` which returns `Option<Duration>`. Requires `use n0_watcher::Watcher` for the `.get()` method.
- **Loss proxy**: `ConnectionStats` has no `lost_packets` field at the connection level. Use `frame_tx.reset_stream / (frame_tx.stream + frame_tx.reset_stream)` as a congestion proxy.

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

### 2026-03-30 — QUIC uni stream multiplexing: moq-lite and raw streams conflict
- **Area**: transport, moq-lite, screen
- **What**: moq-lite (for audio) and our raw screen transport both call `accept_uni()` on the same iroh connection. They steal each other's streams — moq-lite grabs screen streams and resets them ("sending stopped by peer"), while the screen receiver grabs moq-lite streams and fails to parse them. Result: garbled video and most frames dropped.
- **Resolution**: Added a `UniStreamRouter` that reads a 1-byte type tag from each incoming uni stream. Screen streams are tagged with `0x53` ('S'). The router forwards tagged streams to `ScreenReceiver` and everything else to moq-lite via a `NonScreenReceiver` channel. The consumed tag byte is prepended back to moq-lite streams via `WtRecvStream::with_prefix`.
- **Key insight**: Any time two protocols share a QUIC connection, you MUST demux incoming streams. QUIC `accept_uni()` is first-come-first-served with no type discrimination.

### 2026-03-30 — scap captures at Retina resolution, not logical resolution
- **Area**: scap, capture, ffmpeg
- **What**: With `Resolution::Captured`, scap captures at the native pixel resolution of the display. On a Retina MacBook, this is 2x the logical resolution (e.g., 3024x1964 instead of 1512x982). If the encoder is initialized with a fixed 1920x1080 and `fill_bgra_frame` reads the capture data assuming 1920-wide rows, the row stride mismatch causes severe horizontal garbling.
- **Resolution**: Defer scaler initialization until the first frame arrives. Use the actual frame dimensions (`raw.width`/`raw.height`) as the scaler input, and the config dimensions as the output. FFmpeg's `sws_scale` handles the resize + colorspace conversion in one step.
- **Key insight**: Never assume capture dimensions match config dimensions. Always adapt to the actual frame.

### 2026-03-30 — scap sends empty frames on idle status
- **Area**: scap, capture
- **What**: When macOS ScreenCaptureKit reports frame status=1 (idle/no new content), scap sends a `BGRAFrame { width: 0, height: 0, data: vec![] }`. If not filtered, this causes a panic in the encode thread (`range end index 7680 out of range for slice of length 0`).
- **Resolution**: Filter out frames with `width == 0 || height == 0 || data.is_empty()` in `video_frame_to_raw()`.

### 2026-03-30 — postcard is not self-describing
- **Area**: protocol
- **What**: `postcard` serialization format does not include type information. Both sides must have exactly the same type definition to deserialize correctly.
- **Resolution**: Version all protocol types. Include a version field in the session handshake. Never change existing field order — only append new fields.

### 2026-04-02 — hang adoption: unified moq-lite session replaces raw QUIC screen streams
- **Area**: moq-lite, hang, transport
- **What**: P2.1.1 merged screen and audio into ONE moq-lite session with hang framing. Previously, screen used raw QUIC streams (with a custom `ScreenFrameHeader` and `UniStreamRouter` to demux 0x53-tagged screen streams from moq-lite audio streams). This was fragile — two separate transport mechanisms competing for the same QUIC connection's uni streams.
- **Resolution**: Single moq-lite session publishes both `video0` (priority 3) and `audio0` (priority 0) tracks in one broadcast, with a `catalog.json` track describing codec/resolution/bitrate metadata. `hang::container::OrderedProducer` handles group management (keyframe boundaries for video, per-packet groups for audio). `hang::container::OrderedConsumer` provides latency-aware frame reordering on the subscriber side. Deleted: `screen.rs`, `moq_screen.rs`, `moq_audio.rs`, `stream_router.rs`, `ScreenFrameHeader`.
- **Key insight**: hang's `Catalog` auto-names tracks via `Video::create_track("")` → `"video0"`, `Audio::create_track("")` → `"audio0"`. The subscriber reads the catalog to discover track names dynamically — don't hardcode them. hang re-exports `BufList` at `hang::container::BufList`.
