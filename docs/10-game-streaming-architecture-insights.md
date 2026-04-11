# Research 10: Game Streaming Architecture Insights

## Purpose

Analysis of production game streaming systems — Parsec, Sunshine/Moonlight, and Steam Remote Play — to extract architectural patterns applicable to Pairwire. This document focuses on **what they do differently** from Pairwire today and **what is directly actionable**.

---

## The Universal Pipeline

Every game/screen streaming solution follows the same fundamental architecture:

```
Host:
  Capture (GPU surface) → Encode (GPU ASIC) → Packetize → Network
                                                             │
Client:                                                      │
  Render ← Decode (GPU) ← Reassemble ← ─────────────────────┘
    │                                        ↑
    └── Input (kbd/mouse) ──────────────────→┘
```

Differentiation comes from three axes:
1. **How few copies** each step introduces (zero-copy GPU path vs CPU round-trips)
2. **How smart** the network protocol is (FEC, congestion control, feedback loops)
3. **How tight** the encoder-network feedback loop is (adaptive bitrate, selective recovery)

---

## System Profiles

### Parsec

| Aspect | Detail |
|--------|--------|
| **Capture** | DXGI Desktop Duplication API (Windows) — frames captured directly to GPU VRAM. ScreenCaptureKit on macOS. Zero-copy to encoder. |
| **Encode** | Hardware-only: NVENC (NVIDIA), AMF (AMD), VideoToolbox (macOS). H.264 default. No software fallback — Parsec requires a supported GPU. |
| **Transport** | **BUD** — custom protocol built on UDP, encrypted with DTLS 1.2. 97% NAT traversal success rate. Custom congestion control tuned for interactive streaming (not throughput-optimized). |
| **Encoder-Network Coupling** | Tight bidirectional: BUD shares packet loss/congestion info with the encoder in real time. Encoder adjusts bitrate/quality on a per-frame basis. |
| **FEC** | Forward Error Correction to recover lost packets without retransmission round-trip. |
| **Latency** | 4-8ms total pipeline on LAN. Supports up to 240fps. |
| **Key Insight** | Parsec's competitive advantage is the BUD protocol + the feedback loop between encoder and network. The capture/encode path is standard (DXGI + NVENC). The protocol is the moat. |

### Sunshine + Moonlight

| Aspect | Detail |
|--------|--------|
| **Capture (Sunshine)** | Platform-specific: DXGI Desktop Duplication (Windows), NvFBC or KMS/DRM (Linux), ScreenCaptureKit (macOS). NvFBC captures directly to CUDA memory — true zero-copy to NVENC. |
| **Encode (Sunshine)** | NVENC, AMD AMF, Intel Quick Sync, VideoToolbox, VA-API. Software fallback via x264. NVENC path **bypasses FFmpeg's avcodec layer** entirely for lower latency. Async encode added (PR #3629) — capture and encode run concurrently. |
| **Frame Handling** | SIMD-optimized conversion (SSSE3, AVX2, AVX512) for color space transforms. Redundant copies eliminated. VRAM path keeps frames on GPU from capture through encode. |
| **Transport** | NVIDIA GameStream protocol (UDP-based). Designed for LAN; WAN requires VPN (Tailscale/ZeroTier). No built-in NAT traversal. |
| **Decode (Moonlight)** | Hardware decode on client GPU. Frame pacing algorithm smooths jitter — optional 1-frame buffer to reduce micro-stutters. Selective frame dropping when network jitter causes irregular arrival. |
| **Codec Support** | H.264, H.265 (HEVC), AV1. Bitrates up to 150 Mbps. Any resolution/refresh rate the host supports. |
| **Selective Recovery** | NVENC supports selectively invalidating reference frames instead of full IDR keyframes. Recovery with ~2x bandwidth instead of 5-10x. |
| **GPU Power** | Explicitly requests high GPU power mode — NVIDIA's adaptive P-State doesn't detect low-latency streaming as "high load," causing 3-5x slower encoding in power-save mode. |
| **Key Insight** | Sunshine's advantage is the zero-copy VRAM pipeline and async NVENC. It gets ~30% lower CPU usage than FFmpeg's generic avcodec path. Moonlight's frame pacing is what makes it feel smooth. |

### Steam Remote Play

| Aspect | Detail |
|--------|--------|
| **Capture** | Integrated into Steam's overlay — hooks into the game's render pipeline directly. |
| **Encode** | GPU hardware encode: H.264 or HEVC. Codec selected based on client hardware capabilities. |
| **Transport** | Custom low-latency UDP protocol. **Bypasses OS network buffers** — packets pushed directly to the decoder ASAP. ~1ms network latency on wired LAN. |
| **Scope** | Steam games only (or non-Steam shortcuts). Not a general-purpose remote desktop. |
| **Key Insight** | Steam's advantage is hooking into the render pipeline (no separate capture step) and bypassing OS network buffers. Neither is applicable to a general screen-sharing tool. |

---

## Gap Analysis: Pairwire vs These Systems

### Where Pairwire Is Today

```
Capture (scap, BGRA, CPU)
  → copy row-by-row into FFmpeg frame
    → sws_scale BGRA→YUV420P (another copy)
      → H.264 encode (VideoToolbox or libx264)
        → Bytes::copy_from_slice (another copy)
          → moq-lite over QUIC/iroh
```

**3 CPU-side copies per frame** before encoding starts. No feedback from client to host. No frame pacing on the client. Fixed keyframe interval with no on-demand recovery.

### Where These Systems Are

```
Capture (DXGI/NvFBC/SCK → GPU surface, zero-copy)
  → hardware encode (same GPU surface, zero-copy)
    → custom UDP with FEC + adaptive bitrate
      → hardware decode → frame pacing → render
        → receiver reports back to host
```

**0-1 copies** on the hot path. Bidirectional feedback. Adaptive everything.

---

## Actionable Insights

### Insight 1: NV12 Capture — Eliminate Color Conversion

**Problem**: Pairwire captures BGRA from scap, then `sws_scale` converts to YUV420P for the encoder. This is the most expensive per-frame CPU operation.

**What Sunshine does**: Captures in the encoder's native format (NV12/YUV) from the start, or keeps the GPU texture and feeds it directly to the hardware encoder.

**Fix for Pairwire**: scap supports `FrameType::YCbCr` output on macOS (NV12 from ScreenCaptureKit). Switch capture format to NV12 and match the encoder's input format. The scaler becomes a resize-only pass (or is eliminated if capture and encode resolutions match).

```rust
// capture_thread.rs — change output format
let options = Options {
    output_type: FrameType::YCbCrFrame,  // was BGRAFrame
    // ...
};

// encode_thread.rs — match input format
// If scap outputs NV12, set scaler input to Pixel::NV12
// Encoder already outputs YUV420P, so this is NV12→YUV420P (trivial)
// or configure encoder to accept NV12 directly
```

**Impact**: ~30% CPU reduction on the encode thread. Eliminates the most expensive copy.

**Effort**: Small. Scap already supports it. Encoder format matching is straightforward.

**When**: Now.

---

### Insight 2: Keyframe Request Channel — On-Demand Error Recovery

**Problem**: Pairwire sends IDR keyframes on a fixed interval (`keyframe_interval_secs * fps`). When the client joins mid-stream, detects corruption, or recovers from packet loss, it must wait for the next scheduled keyframe. This means up to 2 seconds of black/garbled screen.

**What Sunshine/Parsec do**: The client sends a "request keyframe" message upstream. The encoder responds immediately with an IDR (or, with NVENC, a selective reference frame invalidation that uses ~2x bandwidth instead of 5-10x).

**Fix for Pairwire**: Add a `tokio::sync::mpsc` channel from the session/transport layer to the encode thread. When the subscriber detects a late join, corruption, or the quality monitor detects recovery from packet loss, it sends a keyframe request.

```rust
// In encode_loop — add keyframe request check
if keyframe_request_rx.try_recv().is_ok() {
    yuv.set_kind(picture::Type::I);
    info!("keyframe requested by receiver");
}
```

**Impact**: Instant stream recovery instead of waiting up to 2 seconds. Critical for reconnection scenarios (which iroh handles automatically — the stream resumes but the decoder needs a keyframe).

**Effort**: Small — one channel, one `try_recv()` check per frame, one datagram message type.

**When**: Now.

---

### Insight 3: Receiver Reports — Client-to-Host Feedback

**Problem**: Pairwire's quality adaptation (`quality.rs`) is one-directional: host monitors its own QUIC connection stats (RTT, reset streams) and adjusts encoder config. The host has no idea:
- What the client's decode buffer looks like (full? empty? falling behind?)
- What the true end-to-end latency is (RTT ≠ capture-to-display)
- Whether the client successfully decoded the last keyframe

**What Parsec/Sunshine do**: The client sends periodic "receiver reports" (every ~500ms) containing:
- Last successfully decoded frame timestamp
- Decode queue depth
- Client-side decode latency

The host uses this for smarter bitrate adaptation and knows when recovery is needed.

**Fix for Pairwire**: Define a `ReceiverReport` message (fits in a QUIC datagram, <50 bytes):

```rust
struct ReceiverReport {
    last_decoded_timestamp_us: u64,
    decode_queue_depth: u8,
    decode_latency_us: u32,
}
```

Send via the existing datagram side-channel (same path as cursor updates). The host's `QualityMonitor` incorporates these signals alongside QUIC-level stats.

**Impact**: True end-to-end quality adaptation instead of host-side guessing. Enables smarter bitrate/resolution decisions.

**Effort**: Medium — new message type, datagram send/recv on both sides, integration with `QualityMonitor`.

**When**: Phase 2.

---

### Insight 4: Client-Side Frame Pacing

**Problem**: Pairwire's decode thread outputs frames as fast as they arrive. Network jitter causes uneven frame delivery — sometimes 2 frames in 10ms, sometimes 0 frames for 50ms. This manifests as visible stuttering, especially during scrolling or animation.

**What Moonlight does**: Implements frame pacing with an optional jitter buffer:
- Tracks inter-frame arrival times and expected intervals
- Holds frames that arrive too early (< 0.5x expected interval)
- Drops frames that arrive too late to display
- Optional 1-frame buffer for smoother output at cost of ~16ms latency

**Fix for Pairwire**: Add a frame pacer between decode output and the viewer:

```rust
// Frame pacer logic (simplified)
let expected_interval = Duration::from_micros(1_000_000 / fps as u64);
let mut last_display_time = Instant::now();

loop {
    let frame = decode_rx.recv().await;
    let elapsed = last_display_time.elapsed();

    if elapsed < expected_interval / 2 {
        // Frame arrived too early — hold briefly
        tokio::time::sleep(expected_interval - elapsed).await;
    }

    display_frame(frame);
    last_display_time = Instant::now();
}
```

**Impact**: Smoother visual output. Less noticeable for static code viewing, more noticeable during scrolling, terminal output, or UI navigation.

**Effort**: Medium — new module between decode and render, needs tuning.

**When**: Phase 2.

---

### Insight 5: Async Hardware Encode

**Problem**: Pairwire's encode loop is synchronous: `recv(frame)` → `sws_scale` → `send_frame` → `receive_packet` → loop. While the hardware encoder processes frame N, the encode thread blocks on `receive_packet()`. The capture thread fills its channel (capacity 4) and starts dropping frames.

**What Sunshine does**: Uses NVENC's async mode — submit frame N, immediately start processing frame N+1 from the capture channel. NVENC signals completion via an event handle. On macOS, VideoToolbox's `VTCompressionSessionEncodeFrameWithOutputHandler` provides the same pattern via callback.

**Fix for Pairwire**: Split `encode_loop` into submit and collect phases:

```
Thread A (submit): recv raw frame → scale → send_frame (non-blocking)
Thread B (collect): receive_packet → package → send to network

Or with VideoToolbox callbacks:
Single thread: recv frame → VTCompressionSession.encodeFrame(callback: |packet| { tx.send(packet) })
```

**Impact**: Higher sustained frame rates under load. Prevents frame drops when encoding is momentarily slow (e.g., complex scene change).

**Effort**: Large — requires restructuring the encode thread or using platform-specific async APIs.

**When**: Phase 3.

---

### Insight 6: GPU Power State Hints

**Problem**: NVIDIA's adaptive P-State algorithm doesn't recognize low-latency streaming as a "high load" workload. The GPU stays in a low-power state, causing encoding to take 3-5x longer than it should.

**What Sunshine does**: Explicitly requests high GPU power mode and realtime scheduling priority for NVENC when hardware-accelerated GPU scheduling (HAGS) is enabled.

**Fix for Pairwire**: On macOS, VideoToolbox manages power state internally — no action needed. On Linux/Windows (Phase 3), set GPU power hints at session start via platform APIs.

**Impact**: Prevents intermittent latency spikes from GPU power state transitions.

**Effort**: Small per platform, but platform-specific.

**When**: Phase 3 (when Linux/Windows support is added).

---

## Where Pairwire Is Already Ahead

### Static Frame Detection

Sunshine relies entirely on the H.264 encoder's internal motion estimation to handle static screens. Pairwire hashes every frame (`encode_thread.rs:155-176`) and **skips encoding entirely** when the screen hasn't changed, plus sends keepalive packets. For coding workloads where the screen is static 80%+ of the time, this is a significant CPU and bandwidth advantage.

### QUIC Stream Multiplexing

Game streaming tools use a single UDP stream for video. Pairwire uses QUIC's native stream multiplexing via moq-lite to carry screen, audio, cursor, and control data with **per-stream priority**. This means a large keyframe doesn't block cursor updates — QUIC's priority scheduling handles it. Parsec and Sunshine have to implement this prioritization in their custom protocols.

### Network Resilience

iroh's connection migration and relay fallback give Pairwire better behavior on flaky networks (WiFi→Ethernet transitions, NAT changes) than Sunshine/Moonlight, which have no built-in NAT traversal and require manual VPN setup for WAN.

### Terminal-Native Sharing (Planned)

None of these game streaming tools can share terminal state directly. They all capture pixels. Pairwire's planned PTY forwarding (Phase 4) is a fundamentally different capability — pixel-perfect text at any resolution with minimal bandwidth.

---

## Open Source Projects Reference

| Project | Language | License | What It Does | Relevance |
|---------|----------|---------|-------------|-----------|
| [Sunshine](https://github.com/LizardByte/Sunshine) | C++ | GPL-3.0 | Game stream host (GameStream protocol) | Capture/encode pipeline reference, async NVENC |
| [Moonlight](https://moonlight-stream.org/) | C/Java/Swift | GPL-3.0 | Game stream client (multi-platform) | Decode/render pipeline, frame pacing |
| [Selkies-GStreamer](https://github.com/selkies-project/selkies) | Python/C | MPL-2.0 | WebRTC remote desktop for Linux/K8s | Browser-based streaming, GStreamer pipeline |
| [Wolf (Games on Whales)](https://github.com/games-on-whales/wolf) | C++ | MIT | Stream games from Docker to Moonlight | Containerized streaming architecture |
| [scap](https://github.com/CapSoftware/scap) | Rust | MIT | Cross-platform screen capture | Already used by Pairwire |
| [Apollo/Artemis](https://github.com/ClassicOldSong/Apollo) | C++/Various | Various | Enhanced Moonlight ecosystem fork | Additional client optimizations |

---

## Summary: Priority Action Items

| Priority | Insight | Effort | CPU/Latency Impact | Phase |
|----------|---------|--------|--------------------|-------|
| **P0** | NV12 capture (skip BGRA→YUV) | Small | ~30% CPU reduction | Now |
| **P0** | Keyframe request channel | Small | Instant error recovery | Now |
| **P1** | Receiver reports (client→host) | Medium | True adaptive quality | Phase 2 |
| **P1** | Client-side frame pacing | Medium | Smoother display | Phase 2 |
| **P2** | Async hardware encode | Large | Higher sustained FPS | Phase 3 |
| **P2** | GPU power state hints | Small | Platform-specific gains | Phase 3 |

---

*Last updated: 2026-04-09*
*Sources: Sunshine (LizardByte/Sunshine), Moonlight (moonlight-stream), Parsec technical blog, Steam Remote Play documentation*
