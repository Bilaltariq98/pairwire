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

### 2026-04-02 — Raw QUIC streams conflict with moq-lite on the same connection
- **Area**: transport, moq-lite, QUIC
- **What**: After P2.1.1, screen+audio use moq-lite over the iroh QUIC connection. Attempting to open raw uni streams alongside (e.g., for input forwarding P3.5.2, or cursor datagrams) causes **stream stealing**: both moq-lite's internal `Subscriber::run_uni()` and our `conn.accept_uni()` race on the same `accept_uni()` call. One steals the other's stream silently, causing deadlocks (moq-lite never receives data) or protocol errors (our code receives a moq-lite data stream it can't parse).
- **Resolution**: **Do NOT mix raw QUIC streams with moq-lite on the same connection.** All data that goes over uni/bidi streams must go through moq-lite tracks. Input events, cursor updates, and control messages should be moq-lite tracks (not raw QUIC streams or datagrams). Datagrams (used for cursor) are an exception — QUIC datagrams are a separate channel and don't conflict with stream accept calls.
- **Key insight**: The old `UniStreamRouter` worked around this by tagging raw screen streams with 0x53 and routing everything else to moq-lite. But this is a band-aid. The correct architecture is: **moq-lite owns all streams on the connection**. Use moq-lite tracks for everything. Datagrams are safe. Raw bidi/uni streams are not.
- **Impact on bidirectional audio (P3.2.1)**: Guest→host audio must also be a moq-lite track, not a raw stream. moq-lite supports bidirectional sessions (`Server::with_publish().with_consume()`, `Client::with_publish().with_consume()`), but this needs careful testing — initial attempts had announcement delivery issues that may be timing-related. The recommended approach: add a `control` or `input` moq-lite track alongside the media tracks.

### 2026-04-02 — macOS tray icon: CFRunLoopRunInMode vs NSApplication event pump
- **Area**: macOS, tray-icon, AppKit
- **What**: The `tray-icon` crate creates an `NSStatusItem` with an `NSMenu`. The icon renders correctly when using `NSApplication::setActivationPolicy(Accessory)`, but clicking it does nothing. `CFRunLoopRunInMode()` processes Core Foundation timer/source events but does NOT dispatch AppKit events — NSMenu popups require `[NSApp sendEvent:]`.
- **Resolution**: Replace `CFRunLoopRunInMode` with the standard Cocoa event pump: `NSApplication::nextEventMatchingMask_untilDate_inMode_dequeue()` + `NSApplication::sendEvent()` in a loop. This processes all AppKit events (menu clicks, mouse events, popups) correctly. Drop the `core-foundation` dependency; use `objc2-app-kit` (NSApplication, NSEvent) and `objc2-foundation` (NSDate, NSString) instead.
- **Key insight**: For CLI binaries that need a macOS tray icon without a full windowed app, the three requirements are: (1) `setActivationPolicy(Accessory)` before creating the icon, (2) create the icon on the main thread, (3) pump NSApplication events (not just CFRunLoop) on the main thread.

### 2026-04-03 — moq-lite NEVER reads datagrams — verified in source
- **Area**: moq-lite, QUIC, transport architecture
- **What**: Investigated moq-lite 0.15.5 source to understand which QUIC primitives it uses. The `web_transport_trait::Session` has `recv_datagram()` but moq-lite never calls it. All media (Frames, Groups, Tracks) flows through unidirectional streams only. The datagram methods are pure pass-through for interface completeness.
- **Resolution**: QUIC datagrams are a completely safe side-channel that moq-lite will never interfere with. However, if multiple tasks call `conn.read_datagram()` they steal datagrams from each other non-deterministically. A single DatagramRouter that reads ALL datagrams and dispatches by tag byte is the correct pattern.
- **Key insight**: The transport architecture should be: (1) moq-lite owns all streams, (2) ONE datagram router owns all datagrams, (3) nothing else touches the raw connection primitives.

### 2026-04-03 — core-graphics cursor polling needs `CGEventSource`, not `CGEventType`
- **Area**: macOS, input, core-graphics
- **What**: While adding host cursor polling, `CGEvent::new(CGEventType::Null)` did not compile on `core-graphics 0.25`. The constructor takes a `CGEventSource` and returns `Result<CGEvent, ()>`, not `Option<CGEvent>`.
- **Resolution**: Create a `CGEventSource` with `CGEventSourceStateID::HIDSystemState`, then call `CGEvent::new(source)` and read `.location()` from the resulting event.

### 2026-04-03 — Bidirectional moq-lite (with_origin / with_publish+with_consume) is fragile
- **Area**: moq-lite, transport
- **What**: Attempted multiple approaches to get guest→host audio over moq-lite alongside host→guest screen+audio. `Server::with_origin()` caused the host to see its own broadcast announcement and try to subscribe to itself. `Server::with_publish().with_consume()` + `Client::with_publish().with_consume()` caused the guest to get stuck — the moq-lite session internally opens control streams for both Publisher and Subscriber roles, and the two sides' Publishers/Subscribers fight over stream accept calls.
- **Resolution**: Keep moq-lite **strictly unidirectional** (Server publishes, Client consumes). Use tagged QUIC datagrams for the reverse direction (guest→host audio). This avoids all stream contention. See `tasks/datagram-router.md` for the implementation plan.
- **Key insight**: moq-lite is designed for a relay topology (publisher→relay→subscriber). P2P bidirectional use is not its sweet spot. For 1:1 P2P, use moq-lite for the primary media direction and datagrams for low-bandwidth reverse channels.

### 2026-04-03 — OriginConsumer timing is critical and subtle
- **Area**: moq-lite, transport
- **What**: The guest gets stuck on "Connecting..." if the `OriginConsumer` is created at the wrong time relative to `Client::connect()`. The working pattern (proven multiple times): create origin, clone it into `with_consume(origin.clone())`, call `connect()`, THEN call `origin.consume()` to get the consumer. Creating the consumer BEFORE connect means it's registered on shared nodes before the Subscriber task starts inserting remote broadcasts. Creating it at the wrong time causes it to miss the host's announcement.
- **Resolution**: The exact working sequence is:
  ```rust
  let origin = OriginProducer::new();
  let _session = Client::new()
      .with_consume(origin.clone())  // clone, don't move
      .connect(wt).await?;
  let mut origin_rx = origin.consume();  // AFTER connect
  ```
  On the host side, publish-before-accept works because `origin.consume()` (inside `with_publish()`) replays existing broadcasts via `consume_initial()`.
- **Key insight**: Do not refactor this sequence. It's fragile. Any change to the order of origin.clone() / connect() / origin.consume() breaks it silently (no error, just hangs forever).

### 2026-04-03 — Bidi streams are safe alongside moq-lite; uni streams are not
- **Area**: transport, moq-lite, input
- **What**: moq-lite internally calls `accept_uni()` on the connection. Any other code that also calls `accept_uni()` races with it and steals streams non-deterministically. However, moq-lite does NOT call `accept_bi()`. Bidirectional streams are a separate accept path and safe to use.
- **Resolution**: Use bidi streams (`open_control_stream()` / `accept_bi()`) for the input forwarding channel. Open it after the handshake but before moq-lite starts, to ensure reliable stream establishment.
- **Key insight**: The safe primitives alongside moq-lite are: (1) datagrams (moq-lite never reads them), (2) bidi streams (moq-lite never accepts them), (3) the control bidi opened before moq-lite starts. Do NOT open or accept uni streams outside moq-lite.

### 2026-04-30 — scap requires capturer rebuild to toggle show_cursor — no live API
- **Area**: scap, capture, cursor visibility
- **What**: scap 0.1.0-beta has no API to toggle cursor visibility on a running capturer. The `Options` struct accepts a `show_cursor: bool` field but it is consumed at capturer construction; there is no equivalent of `setCapturesCursor` on the live `Capturer` handle.
- **Resolution**: Implemented `CursorVisibilityManager` in `crates/capture/src/cursor_visibility.rs` which stops the current capture thread, drops the old capturer, and spawns a new one with the updated `show_cursor` setting. The new capturer is swapped in via a `watch` channel signalled from `capture_thread.rs`. See `crates/capture/src/capture_thread.rs` for the restart logic.
- **Key insight**: Expect a ~33–500ms video hitch each time control is granted or revoked — this is the scap teardown + ScreenCaptureKit re-authorisation window. On an M-series Mac the hitch is typically under 100ms. There is no way to avoid it without patching scap to expose a live cursor toggle on the underlying `SCStreamConfiguration`.

### 2026-04-30 — tokio broadcast recv() returns Lagged — must continue, not break
- **Area**: tokio, async, broadcast channel
- **What**: `tokio::sync::broadcast::Receiver::recv()` returns `Err(RecvError::Lagged(n))` when the consumer falls behind and some messages are dropped. In the HUD control-state relay task (T7), the initial implementation treated any `Err` as a fatal channel-closed signal and broke out of the receive loop — causing the HUD to freeze in whatever state it was in before the lag.
- **Resolution**: Match `Err(RecvError::Lagged(_))` explicitly and `continue` the loop; only `break` on `Err(RecvError::Closed)`. Pattern used in `crates/app/src/hud.rs` and the host-side tray-state relay.
- **Key insight**: Broadcast channels are lossy by design — a slow consumer will skip messages. For UI state (HUD labels, tray item enable/disable), skipping intermediate transitions is fine as long as the loop survives and picks up the next update. Breaking on lag causes the UI to silently diverge from actual session state.

### 2026-04-30 — iroh open_bi does not announce the stream until the opener writes
- **Area**: iroh, transport, control channel
- **What**: Calling `conn.open_bi()` on the guest side allocates a QUIC stream ID but does NOT send any data to the network. The host's `accept_bi()` does not unblock until the opener actually writes bytes into the send half. This is a QUIC-level behaviour: stream IDs are assigned locally and only become visible to the peer on the first `STREAM` frame.
- **Resolution**: After opening the persistent control bidi stream (`ControlChannel::spawn`), the guest immediately sends an initial `ControlMessage::SetSharing` flush. This forces the first write and unblocks the host's `accept_bi`. See `crates/transport/src/control_channel.rs` for the flush logic and `crates/app/src/session.rs` for the call site.
- **Key insight**: Whenever you open a bidi or uni stream and then `await` something on the remote side, ensure you write at least one byte first. The 250ms `sleep` guard in `session.rs` is a belt-and-suspenders fallback for this; the real fix is the initial write.

### 2026-05-04 — Host overlay: transparent egui windows not practical; mirror window chosen
- **Area**: egui, eframe, macOS, host overlay
- **What**: eframe/winit do not expose the `NSWindow` flags needed for per-pixel transparency (e.g. `setOpaque(NO)` + `setBackgroundColor(NSColor.clear)`) without forking the crate. Attempts to set window level or transparency via `ViewportBuilder` do not produce a fully transparent canvas over the desktop.
- **Resolution**: Use a "mirror window" design: the capture thread's raw frames are teed (via a `std::sync::mpsc::sync_channel` bridge) into both the encode pipeline and a `watch::Sender<Option<DecodedFrame>>`. The overlay window renders this preview frame and draws annotation strokes on top — identical to the guest viewer. The host sees their own screen inside the overlay and annotates there.
- **Key insight**: `RawFrame` (BGRA) can be converted directly to `DecodedFrame` (also BGRA) without encoding/decoding. The tee does a `clone()` of `Bytes` which is cheap (Arc refcount bump, no copy).

### 2026-05-04 — eframe windows on non-main threads with macOS tray
- **Area**: eframe, winit, macOS, tray
- **What**: The macOS tray event loop requires the main thread for NSApplication event dispatch. eframe/winit also prefer the main thread for window creation. When both are needed, the overlay window is launched on a secondary OS thread. This works because by the time `run_host_overlay` is called, the tray has already initialised NSApplication via `NSApplication::sharedApplication(mtm)`, so winit can attach its window to the existing application without needing to be first on the main thread.
- **Resolution**: Launch overlay via `std::thread::spawn`. If a future winit version breaks this, add `--no-overlay` to the tray mode and document it.
- **Key insight**: `with_always_on_top()` in `ViewportBuilder` keeps the overlay above other windows, which is important for annotating a shared screen.

### 2026-05-04 — #[cfg(feature = "gui")] function parameters with run_host
- **Area**: Rust, cfg, function signatures
- **What**: Adding `#[cfg(feature = "gui")]` to a function parameter compiles correctly in Rust — callers must wrap the argument with the same cfg attribute. This is less ergonomic than having two separate functions but avoids code duplication.
- **Resolution**: Used this pattern for `overlay_tx` in `run_host`. The `#[cfg(not(feature = "gui"))] _overlay_tx: Option<()>` fallback parameter keeps the call sites symmetric.
- **Key insight**: Prefer this pattern over feature-gating entire functions when the function is large and only one parameter varies by feature.

### 2026-05-04 — watch::Receiver::wait_for is !Send across await points (do not use in spawned tasks)
- **Area**: tokio, async, watch channel
- **What**: `tokio::sync::watch::Receiver::wait_for(predicate)` returns a `WaitForFut<'_, T>` whose `Output` is a `Result<Ref<'_, T>, RecvError>`. The `Ref<'_, T>` type holds an `RwLockReadGuard<'_, T>`. `RwLockReadGuard` is `!Send`. If you use `wait_for()` in a `select!` branch inside a `tokio::spawn`-ed future, rustc correctly rejects it with "future cannot be sent between threads safely".
- **Resolution**: Use `receiver.changed().await` in a loop, then check `*receiver.borrow()` to read the value. The borrow is taken (and dropped) synchronously at the check point — it never crosses an await boundary. Pattern:
  ```rust
  let shutdown_fut = async move {
      loop {
          if rx.changed().await.is_err() { return; }
          if *rx.borrow() { return; }
      }
  };
  tokio::pin!(shutdown_fut);
  tokio::select! {
      _ = some_async_op() => {},
      () = &mut shutdown_fut => {},
  }
  ```
  For single-use select! branches (no loop), inline the same pattern in a dedicated async block.
- **Key insight**: `wait_for()` is convenient for `block_on` contexts on the main thread but unsafe in `rt.spawn()` futures. Prefer `changed()` + `borrow()` everywhere for consistency.

### 2026-05-04 — Host does not support guest reconnect today; iroh connection is 1:1 per `run_host` invocation
- **Area**: reconnect, session, drawing
- **What**: The guest has a full reconnect loop (`reconnect.rs` / `with_reconnect`) that re-calls `run_join` on disconnect. The host has NO equivalent loop — `run_host` calls `transport.accept()` exactly once, processes one guest, then returns. When the guest reconnects after a drop, it dials a new iroh connection to the same endpoint. The host endpoint (iroh `Endpoint`) does stay alive after the first guest disconnects (the `Endpoint` is not closed until `transport.close()` at the end of `run_host`), but `run_host` never calls `accept()` a second time, so the reconnecting guest's TCP-style SYN goes unacknowledged and the reconnect fails.
- **Impact**: The stroke cache (`StrokeCache`) built in feat/drawing-replay-on-reconnect is fully correct and ready: strokes are accumulated persistently across the session, `replay_persistent_strokes` fires immediately on the first (and currently only) guest connect, and the cache+replay path is wired such that a future host-reconnect loop needs zero changes to the cache itself.
- **Resolution / Next step**: Wrap the `accept → handshake → session` block in a `loop` inside `run_host`. The `stroke_cache` variable is declared above the loop so it naturally survives across iterations. After accepting the annotation stream on each new guest, call `replay_persistent_strokes(&stroke_cache, &draw_event_tx).await` — the call is already there and the doc comment in `drawing.rs` spells out exactly where.
- **Key insight**: iroh's `Endpoint` is reusable — calling `endpoint.accept().await` again after the first connection closes is perfectly valid and will accept the next inbound connection. No re-bind needed.

### 2026-05-04 — Host accept loop + session-state watch + overlay status banner (PR-B)
- **Area**: session, reconnect, overlay, gui
- **What**: Implemented the host accept loop in `feat/host-reconnect-loop`. Key design decisions:
  - **Capture lifetime**: The capture thread + tee thread are started once on the first loop iteration and kept alive across reconnects. An `Arc<Mutex<Option<SyncSender<RawFrame>>>>` (`encode_tx_slot`) lets the tee thread forward frames only when an encoder is wired. Between connections the tee drops frames silently via `try_send` — no spam logging.
  - **Tray relay**: The original `tray_action_rx: Option<mpsc::Receiver<...>>` can only be consumed once. To give each loop iteration a fresh subscription, the relay uses a `tokio::sync::broadcast::Sender`. Each iteration subscribes with `.subscribe()` and bridges broadcast → per-conn mpsc.
  - **Overlay lifetime**: `host_strokes_watch_tx`, `host_live_pt_watch_tx`, `overlay_drawing_bcast_tx`, and `session_state_tx` are all created before the loop and survive across reconnects. The overlay holds the receivers throughout the host process lifetime.
  - **Second-guest rejection**: While a guest is connected, the inner wait loop uses `tokio::select!` between `conn.closed()` and `transport.accept()`. New connections are `drop()`ped immediately with a debug log (`TODO(multi-guest)`).
  - **Overlay auto-close**: `session_state_tx` is dropped when `run_host` returns. The overlay detects `state_rx.has_changed().is_err()` (sender dropped) and issues `ctx.send_viewport_cmd(ViewportCommand::Close)`.
  - **Status banner**: `render_session_banner` paints a small semi-transparent pill at top-center. Showing "Waiting for guest to connect…" or "Connected: <peer_id_short>".
- **watch::Receiver::has_changed()**: Returns `Err` when the sender is dropped. Safe to use as a channel-closed detector without blocking.
- **teardown helpers**: `teardown_drawing`, `teardown_clipboard`, `teardown_input`, `teardown_audio` extracted to avoid duplication between the normal loop teardown and the early-exit (transport closed) path.

### 2026-06-15 — Tray + transparent overlay can coexist in one process (gui+tray)
- **Area**: app, gui, tray, host_overlay_transparent, session
- **What**: Previously `tray` and `gui` were mutually-exclusive cargo features (`tray.rs` was `#[cfg(all(feature = "tray", not(feature = "gui")))]`). In the default (tray, no gui) build the host ran `run_host_with_tray` with a hand-rolled `NSApplication` event pump and launched NO overlay — so the host could never see annotations ("can't see the overlay when the tray is active"). Root cause: on macOS both `NSStatusBar` (tray) and winit's `EventLoop` (eframe) demand the main thread, and `eframe::run_native` seizes it permanently, so the two could not run as separate loops.
- **Resolution**: Unified them. `tray.rs` now compiles under `#[cfg(feature = "tray")]`. A new `run_host_with_overlay_and_tray` (`#[cfg(all(feature = "gui", feature = "tray"))]`) spawns the session with both invite/control plumbing AND overlay channels, then launches `run_transparent_host_overlay_with_tray`. The overlay creates the `TrayHandle` **inside the eframe creation closure** (main thread, `NSApp` already live) and polls it each `update()` via `tray.handle.poll()`, dispatching CopyInvite→clipboard, Disconnect/Quit→`shutdown_tx` + `ViewportCommand::Close`, Grant/Revoke→`control_tx`.
- **Key insight — tray_icon is built to live inside a winit loop**: `tray_icon::menu::MenuEvent::receiver()` is a global crossbeam channel pollable from anywhere, including the eframe `update()`. The `NSStatusItem` must be created on the main thread after `NSApp` exists — the eframe creation closure satisfies this. No separate event loop needed.
- **Key insight — send overlay channels BEFORE the accept loop**: `overlay_tx` used to fire only after the first guest connected (inside the accept loop, when capture started), so the overlay (and now the tray) appeared only once a guest joined — useless for a tray whose job is copying the invite link *before* anyone connects. The overlay never renders the self-preview (`frame_rx` is dropped), so the late `frame_rx` it waited for was unnecessary. Channels are now sent once, before the loop, using `capture_config.width/height` (known early) and an empty preview watch. This also means the host overlay shows its "Waiting for guest…" banner immediately on start in gui-only builds too.
- **cfg gotcha**: `run_transparent_host_overlay` (plain) is now `#[cfg(not(feature = "tray"))]` and `run_host_with_overlay` is `#[cfg(all(feature = "gui", not(feature = "tray")))]`, else they trip `dead_code` under `-D warnings` in the gui+tray combo. `TrayHandle::copy_item` / `set_invite_url` are `#[allow(dead_code)]` (used only by the tray-only host loop). NOTE: `just lint` only checks default features (tray, no gui), so gui/gui+tray lint regressions are invisible there — check `--features gui,tray` explicitly.

### 2026-06-18 — iroh-relay port matrix: TCP 443 WebSocket required, UDP optional, STUN dead
- **Area**: iroh, relay, hosting, deployment
- **What**: While scoping a self-hosted relay for ship, verified iroh-relay's actual network requirements (workspace pins `iroh` + `iroh-relay` at **0.97.0** per Cargo.lock; an earlier assumption that it "needs raw UDP STUN/QAD on 3478" was WRONG):
  - **STUN is retired.** iroh switched to QAD (QUIC Address Discovery) exclusively in **0.90**; iroh-relay 0.97 has no STUN server and UDP 3478 is unused.
  - **Relay forwarding runs over WebSocket-over-HTTPS (TCP 443) only** since **0.91** (raw-TCP and QUIC relay transports were removed — WebSocket is the sole relay transport). When a direct path fails, iroh tunnels the peers' QUIC packets *opaquely* over this WebSocket — the relay is content-blind and never sees moq-lite/hang framing, so **relay choice is fully decoupled from the media protocol**.
  - **QAD is a separate, OPTIONAL UDP server** (default port **7842**, `QuicConfig` in `iroh_relay::server::ServerConfig`). `quic: None` disables it and the relay still starts and forwards traffic; you only lose this host as a QAD address-discovery endpoint (n0's public relays already provide QAD).
- **Resolution**: A self-hosted relay needs a VPS with a public IP and inbound **TCP 443** (required) + **UDP 7842** (recommended, for QAD). Hetzner CX22 fits. **Cloudflare Containers cannot host it** — all container ingress is proxied through a Worker ("end-users cannot make non-HTTP TCP or UDP requests to a Container instance"), which breaks the e2e-encrypted WebSocket QUIC tunnel and blocks UDP entirely. **Cloudflare's managed MoQ relay is irrelevant** — it speaks IETF moq-transport (draft-07→14), not moq-lite, and sits at the wrong layer (would require replacing iroh). Fly.io supports raw UDP + dedicated IPv4 but egress is ~$0.02/GB vs Hetzner's 20 TB included — pricier for relayed video.
- **Key insight**: Wire the relay via `Endpoint::builder().relay_mode(RelayMode::Custom(vec![relay_url]))`. Keep n0's public relays as a fallback so a relay outage degrades (more relay-via-public / slower hole-punch) rather than hard-fails.

### 2026-06-18 — Telemetry consent gate must use --test-threads=1
- **Area**: telemetry, testing
- **What**: `consent::reset_for_test()` mutates process-global `AtomicBool` statics (`CONSENT`, `CONSENT_SET`). When `cargo test` runs multiple consent tests in parallel, a `reset_for_test()` in one test can race with an `is_consented()` check in another, causing intermittent failures.
- **Resolution**: Run `pairwire-telemetry` tests with `--test-threads=1`. In CI, add `RUST_TEST_THREADS=1` for this crate only or use `cargo test -p pairwire-telemetry -- --test-threads=1`.
- **Note**: The production code uses `Ordering::Relaxed` intentionally — a transient wrong consent read costs at most one extra/missed telemetry event, which is acceptable. The test isolation problem is a testing concern, not a production concern.

### 2026-06-18 — sentry crate: pin to exact version, use before_send for consent
- **Area**: telemetry, sentry
- **What**: `sentry` is pre-1.0 and breaks on minor bumps. Also, setting `enabled: false` at init time prevents re-enabling at runtime — the correct GDPR pattern is to leave Sentry enabled but gate events via `before_send` so consent can be toggled mid-session. The panic hook is auto-registered by the `panic` feature flag — no explicit `register_panic_handler()` call is needed.
- **Resolution**: Pin to exact version (verified: `=0.48.2` at 2026-06-18). Use `before_send` callback that calls `pairwire_telemetry::consent::is_consented()` and returns `None` (drop) when not consented. Never use `ClientOptions { enabled: false }` for the consent gate.

### 2026-06-18 — posthog-rs version: 0.4 is not published; use 0.13.x tuple shorthand
- **Area**: telemetry, posthog
- **What**: `posthog-rs = "0.4"` is not published on crates.io. The latest stable version at 2026-06-18 is `0.13.1`. `(&str, &str)` implements `From<(&str, &str)> for ClientOptions` — the idiomatic pattern is `posthog_rs::client((api_key, host_url))` with the tuple shorthand; no `ClientOptions::new()` call needed. See `crates/telemetry/src/posthog.rs:84`.
- **Resolution**: Pin to `"0.13.1"`. Always verify with `cargo search posthog-rs` before adding to `Cargo.toml`. Verify method signatures at implementation time with `cargo doc -p posthog-rs --open`.

### 2026-06-18 — ConnectionEstablished telemetry reports relayed=false unconditionally (known gap)
- **Area**: telemetry, session
- **What**: `relayed: false` is hardcoded in both the host and guest `ConnectionEstablished` event paths (`crates/app/src/session.rs:329` and `:1156`). iroh does expose path information via `conn.remote_info()` / `DirectAddrInfo`, but extracting relay vs. direct status requires querying the connection after the handshake and is not yet wired. Until the TODO is resolved, PostHog will report 0% relay connections even when relay is actually used — the data is factually incorrect for relayed sessions.
- **Resolution**: When implementing relay detection, use `iroh::endpoint::Connection::remote_info()` to inspect the path; if `remote_info.relay_url` is `Some` and no direct addresses are reachable, `relayed = true`. Update both call sites and the `TelemetryEvent::ConnectionEstablished` shape if a `relay_url` field is also wanted.

### 2026-06-18 — egui 0.31: no Frame::window() — use Frame::NONE.fill()
- **Area**: UI, egui
- **What**: `egui::Frame::window(&ctx.style())` does not exist in egui 0.31. The workspace uses egui 0.31 (`Cargo.toml:48`). The correct pattern for a styled frame in egui 0.31 is `egui::Frame::NONE.fill(ctx.style().visuals.panel_fill)`. This is used at `host_overlay_transparent.rs:858` and `viewer.rs:749`.
- **Resolution**: Always check existing files for egui API usage before writing new egui widget code. Do not reference egui docs for versions other than 0.31.
