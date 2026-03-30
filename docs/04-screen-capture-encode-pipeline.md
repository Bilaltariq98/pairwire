# Research 4: Screen Capture + Encode Pipeline in Rust

## Screen Capture Crates

### scap (CapSoftware/scap) — Recommended

The most actively maintained cross-platform screen capture library in Rust. Built by the team behind Cap (a screen recording app).

**Current version**: 0.1.0-beta.1 (August 2025)

**Platform backends**:
- **macOS**: ScreenCaptureKit (Apple's modern API, supports permission prompts)
- **Windows**: windows-capture crate (Desktop Duplication API)
- **Linux**: PipeWire

**Features**:
- Up to 60fps capture
- Display or window targeting
- Cursor overlay
- Crop/region selection
- Resolution scaling (720p, 1080p, native)
- Output formats: BGRA, YCbCr (NV12)
- Permission checking and requesting
- Excluded targets (hide your own app window)

**API**:
```rust
use scap::{capturer::{Capturer, Options}, frame::Frame};

let options = Options {
    fps: 60,
    target: None,  // Primary display
    show_cursor: true,
    output_type: scap::frame::FrameType::BGRAFrame,
    output_resolution: scap::capturer::Resolution::_1080p,
    ..Default::default()
};

let mut capturer = Capturer::build(options)?;
capturer.start_capture();
// Frames arrive via callback or polling
```

**Limitations**: Beta quality. Documentation is sparse. The macOS backend has some edge cases with window capture. No dirty-rectangle support (captures full frame every time).

### scrap — Legacy

The original Rust screen capture crate. Simple API but hasn't been significantly updated since 2018. Uses older platform APIs (CGDisplayStream on macOS, Desktop Duplication on Windows, X11 on Linux). No PipeWire support. Still works but scap is a better choice for new projects.

### xcap — Alternative

The `screenshots` crate redirects to xcap. Takes static screenshots (single frames) rather than continuous capture. Not suitable for real-time streaming.

## Hardware Video Encoding

This is the hardest part of the pipeline. Rust doesn't have a mature, cross-platform hardware encoding crate. Your options:

### FFmpeg Bindings (ffmpeg-next, ffmpeg-sys-next)

The pragmatic choice. FFmpeg supports all hardware encoders:
- **NVENC** (NVIDIA): H.264, H.265, AV1
- **VideoToolbox** (macOS): H.264, H.265
- **VA-API** (Linux): H.264, H.265
- **QSV** (Intel): H.264, H.265, AV1
- **AMF** (AMD): H.264, H.265, AV1
- **Media Foundation** (Windows): H.264, H.265

```rust
// Pseudocode using ffmpeg-next
let encoder = ffmpeg::encoder::find_by_name("h264_videotoolbox")?;
let mut context = encoder.video()?;
context.set_width(1920);
context.set_height(1080);
context.set_time_base((1, 60));
context.set_pixel_format(Pixel::NV12);
// Low-latency tuning
context.set_option("realtime", "1")?;
context.set_option("allow_sw", "0")?;  // Hardware only
```

**Tradeoff**: FFmpeg is a C dependency. Adds ~20MB to binary, requires linking. But it's battle-tested and handles all the hardware encoder quirks.

### GStreamer Bindings (gstreamer-rs)

Similar to FFmpeg — wraps a C library. moq-dev already has a GStreamer plugin for publishing MoQ broadcasts. Could be a good fit if you want to align with moq's existing tooling.

### Pure Rust Options

- **openh264**: Rust bindings to Cisco's OpenH264. Software-only, not suitable for real-time screen sharing at high resolutions.
- **rav1e**: AV1 encoder in pure Rust. High quality but too slow for real-time screen sharing without hardware acceleration.
- **x264/x265 bindings**: Exist but are software encoders.

**Reality check**: For a production screen-sharing app, you NEED hardware encoding. There is no way around the FFmpeg/GStreamer dependency for this.

## Frame Delta / Dirty Region Detection

### Capture-Level Dirty Rects

- **macOS ScreenCaptureKit**: Provides `dirtyRects` in frame metadata, listing which rectangles changed. This is gold for screen sharing.
- **Windows Desktop Duplication**: Provides dirty rects and move rects natively.
- **Linux PipeWire**: Provides damage regions on Wayland.

scap does NOT currently expose dirty-rect information from these APIs. You'd need to either:
1. Fork scap and expose the dirty-rect metadata
2. Use platform APIs directly
3. Implement frame differencing in software

### Software Frame Differencing

If dirty rects aren't available:
```rust
// Compare current frame to previous frame, block by block
// 16x16 or 32x32 blocks
for block in frame.blocks(32, 32) {
    if block != previous_frame.block_at_same_position() {
        changed_blocks.push(block);
    }
}
```

This is how RDP and VNC work. It's effective but means reading every pixel every frame (CPU cost).

### Combining with Hardware Encoding

Option A: **Full-frame encode, always**
- Simplest. Feed every captured frame to the hardware encoder.
- The encoder's own motion estimation handles temporal redundancy.
- H.264/H.265 inter-frame compression is quite good for screen content.
- This is likely what Tuple does.

Option B: **Skip unchanged frames**
- Only submit frames to the encoder when dirty rects indicate changes.
- When screen is static (user reading code), zero bandwidth.
- When screen changes, full frame encode.
- Significant bandwidth savings for typical coding sessions.

Option C: **Region-of-interest encoding**
- Use dirty rects to set ROI for the encoder (higher quality in changed regions).
- More complex, hardware encoder support varies.

**Recommendation**: Start with Option A (always encode) + Option B (skip static frames). This is simple and effective. Dirty-rect-based partial updates are an optimization for later.

## Codec Selection for Screen Sharing

### H.264 (AVC)

- **Pro**: Universal hardware encode/decode support. Lowest encode latency. Best compatibility.
- **Con**: Lower compression efficiency than H.265/AV1.
- **Screen content**: H.264 doesn't have a dedicated screen content mode, but "high" profile with appropriate settings works well.
- **Latency**: ~2-5ms encode on hardware, ~1-3ms decode.
- **This is the right choice for MVP.**

### H.265 (HEVC)

- **Pro**: ~30-40% better compression than H.264 at same quality. Screen Content Coding extensions in HEVC (palette mode, IBC).
- **Con**: Not universally supported. Patent licensing complexity. Slightly higher encode/decode latency.
- **Good for phase 2** when you want to reduce bandwidth.

### AV1

- **Pro**: Best compression. Royalty-free. Screen Content Coding tools built-in.
- **Con**: Hardware encode support is still emerging (newest GPUs only). Higher latency. Software decode is feasible but burns CPU.
- **Too early for a screen sharing product** unless you only target recent hardware.

## The Full Pipeline

```
Thread Architecture:

┌─────────────────┐
│  Capture Thread  │  scap runs its own callback thread
│  (platform)      │  Outputs: BGRA or NV12 frames + timestamps
└────────┬────────┘
         │ frame channel (ring buffer, drop oldest if full)
         ▼
┌─────────────────┐
│  Encode Thread   │  FFmpeg hardware encoder
│  (dedicated)     │  Input: raw frames
│                  │  Output: H.264 NAL units
│                  │  Logic: skip static frames, keyframe interval
└────────┬────────┘
         │ encoded packet channel
         ▼
┌─────────────────┐
│  Network Thread  │  iroh/moq-lite
│  (tokio runtime) │  Packetize into MoQ groups/frames
│                  │  Manage streams, datagrams, priority
└─────────────────┘

Decode side (receiving):

┌─────────────────┐
│  Network Thread  │  Receive MoQ groups/frames
│  (tokio runtime) │  Reassemble encoded packets
└────────┬────────┘
         │ encoded packet channel
         ▼
┌─────────────────┐
│  Decode Thread   │  FFmpeg hardware decoder
│  (dedicated)     │  Output: raw frames
└────────┬────────┘
         │ decoded frame channel
         ▼
┌─────────────────┐
│  Render Thread   │  UI framework (egui/iced/wgpu)
│  (main thread)   │  Display remote screen + overlays
└─────────────────┘
```

### Minimizing Copies

1. scap outputs BGRA or NV12 directly — if encoder accepts NV12, skip color conversion
2. Use GPU textures where possible (scap on macOS can output IOSurface, which VideoToolbox accepts directly — zero-copy)
3. Encoded NAL units are small enough that copies are cheap
4. On decode side, hardware decoder can output directly to GPU texture for rendering

### Variable Frame Rate

```rust
// In encode thread
loop {
    let frame = capture_rx.recv().await?;
    
    // Compare to previous frame (quick hash or dirty rect check)
    if frame_is_identical_to_previous(&frame, &prev_frame) {
        continue;  // Don't encode static screen
    }
    
    // Keyframe every N seconds or on significant change
    let force_keyframe = last_keyframe.elapsed() > Duration::from_secs(2);
    
    let packet = encoder.encode(&frame, force_keyframe)?;
    network_tx.send(packet).await?;
    
    prev_frame = frame;
}
```

## Audio Capture

### cpal (Cross-Platform Audio Library)

The standard Rust audio crate. Supports:
- macOS (CoreAudio)
- Windows (WASAPI)
- Linux (ALSA, PulseAudio, JACK)

Captures microphone input. System audio capture (loopback) is platform-specific and less reliable.

### Opus Encoding

Use the `opus` or `audiopus` crate. Opus is ideal for voice:
- 20ms frame size standard
- 32kbps is excellent quality for voice
- Built-in voice activity detection
- Low latency (~25ms algorithmic delay)

### Echo Cancellation

This is the unsolved problem in pure Rust. Options:
- **WebRTC audio processing module**: C++ library, would need FFI bindings. This is what everyone uses (Zoom, Discord, Tuple). No pure Rust equivalent exists.
- **SpeexDSP**: C library with Rust bindings (`speexdsp` crate). Has echo cancellation but lower quality than WebRTC's.
- **Skip it for MVP**: If users wear headphones (most developers do), echo cancellation isn't needed. Add it in phase 2/3.

## What Tuple Likely Does

Based on their claims:
- **Natively compiled C++ engine**: Direct access to platform APIs, no Electron overhead
- **5K streaming**: Hardware encoding is mandatory at this resolution
- **Low CPU usage**: Efficient capture (ScreenCaptureKit/Desktop Duplication) + hardware encode
- **End-to-end encryption**: Custom transport, not WebRTC (since they don't mention it)
- **Cross-platform**: Likely uses platform-specific capture + encoding backends behind a unified interface
- **"Not just another Electron app"**: Their core differentiator is performance

They've been iterating for years. Their pipeline is likely: ScreenCaptureKit → VideoToolbox H.264 → custom transport → VideoToolbox decode → Metal render. The Rust equivalent would be: scap → FFmpeg VideoToolbox → iroh/moq-lite → FFmpeg decode → wgpu render.
