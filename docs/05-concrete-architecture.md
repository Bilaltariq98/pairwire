# Research 5: Concrete Architecture — Putting It All Together

## Recommended Stack Decision

| Layer | Choice | Why |
|---|---|---|
| Connectivity | **iroh** (uses noq) | NAT traversal, hole-punching, relay fallback, EndpointId addressing |
| Media Transport | **moq-lite** (via moq-dev/moq) | Priority scheduling, group/frame model, iroh integration exists |
| Media Framing | **hang** (from moq-dev/moq) | Catalog, codec metadata, track management |
| Screen Capture | **scap** | Cross-platform, modern APIs, active development |
| Video Encode/Decode | **FFmpeg** (via ffmpeg-next) | Hardware accel on all platforms, battle-tested |
| Audio Capture | **cpal** | Cross-platform, well-maintained |
| Audio Codec | **opus** (via audiopus) | Low latency, excellent voice quality |
| UI Framework | **iced** or **egui** | Pure Rust, GPU-accelerated, cross-platform |
| Serialization | **postcard** or **bincode** | Compact binary, fast, serde-compatible |
| Async Runtime | **tokio** | Required by iroh and quinn |

## Process Model

```
Single process, multi-threaded:

┌──────────────────────────────────────────────────────┐
│                    Main Process                       │
│                                                       │
│  ┌─────────┐  ┌──────────┐  ┌──────────┐            │
│  │ UI/     │  │ Capture  │  │ Audio    │             │
│  │ Render  │  │ Thread   │  │ Thread   │             │
│  │ Thread  │  │          │  │          │             │
│  │ (main)  │  │ scap +   │  │ cpal +   │             │
│  │         │  │ FFmpeg    │  │ opus     │             │
│  │ iced/   │  │ encode   │  │ encode   │             │
│  │ egui +  │  │          │  │          │             │
│  │ wgpu    │  │ FFmpeg    │  │ opus     │             │
│  │         │  │ decode   │  │ decode   │             │
│  └────┬────┘  └────┬─────┘  └────┬─────┘            │
│       │            │              │                   │
│       │     ┌──────┴──────────────┴──────┐           │
│       │     │                            │           │
│       └─────┤    Network / Protocol      │           │
│             │    Thread (tokio)           │           │
│             │                            │           │
│             │  iroh Endpoint             │           │
│             │  ├── moq-lite session      │           │
│             │  │   ├── screen track      │           │
│             │  │   └── audio track       │           │
│             │  ├── control stream (raw)  │           │
│             │  ├── input stream (raw)    │           │
│             │  └── datagrams (cursor)    │           │
│             └────────────────────────────┘           │
└──────────────────────────────────────────────────────┘
```

### Why Single Process

- Avoids IPC overhead for real-time data
- Shared memory between threads (zero-copy frame passing with Arc<[u8]>)
- Simpler deployment (one binary)
- iroh's Endpoint is designed as a singleton per process

### Thread Communication

```rust
// Between capture and network: bounded channel, drop oldest on overflow
let (frame_tx, frame_rx) = tokio::sync::mpsc::channel::<EncodedFrame>(4);

// Between network and decoder: bounded channel
let (decode_tx, decode_rx) = tokio::sync::mpsc::channel::<EncodedFrame>(8);

// Between decoder and UI: single-slot (only latest frame matters)
let latest_frame = Arc::new(AtomicCell::new(None::<DecodedFrame>));

// Datagrams (cursor): crossbeam channel for low-latency
let (cursor_tx, cursor_rx) = crossbeam_channel::bounded::<CursorPosition>(1);
```

## Crate Dependency Graph

```
your-app
├── iroh                      # Connectivity + NAT traversal
│   └── noq                   # QUIC implementation (internal)
├── moq-lite                  # Media pub/sub transport
│   └── moq-native            # QUIC/WebTransport helpers (uses Quinn or noq)
├── hang                      # Media framing (catalog, tracks, codecs)
├── scap                      # Screen capture
│   ├── screencapturekit      # macOS backend
│   ├── windows-capture       # Windows backend
│   └── pipewire              # Linux backend
├── ffmpeg-next               # Video encode/decode
├── cpal                      # Audio capture
├── audiopus                  # Opus encode/decode
├── iced (or egui)            # UI framework
│   └── wgpu                  # GPU rendering
├── tokio                     # Async runtime
├── serde + postcard          # Serialization
├── tracing                   # Logging/diagnostics
└── clap                      # CLI (optional)
```

## Protocol Design

### Session Establishment

```
Peer A (Host):
1. Generate or load SecretKey → EndpointId
2. Create iroh Endpoint
3. Generate session invite: {endpoint_id, relay_url, session_token}
4. Encode as URL: pairprog://join?id=<base32>&relay=<url>&token=<jwt>
5. Share via clipboard/link/QR

Peer B (Guest):
1. Create iroh Endpoint
2. Parse invite URL
3. Connect to Peer A via iroh (automatic NAT traversal)
4. Exchange capabilities on control stream:
   {
     version: 1,
     capabilities: ["screen", "audio", "remote_control", "annotations"],
     display: { width: 2560, height: 1440, scale: 2.0 },
     codecs: ["h264", "opus"]
   }
5. Negotiate session parameters
6. Start media tracks
```

### Screen Sharing Protocol

```
MoQ Track: "screen"
Priority: 3
Group structure:
  - Each keyframe starts a new group
  - Delta frames are appended to the current group
  - Keyframe interval: 2 seconds (or on significant scene change)

Frame format (within moq-lite frame):
  Header (8 bytes):
    - timestamp: u48 (microseconds)
    - flags: u8 (keyframe, last_in_group, etc.)
    - reserved: u8
  Payload:
    - H.264 NAL unit(s)

Quality adaptation:
  - Monitor RTT and loss via QUIC stats
  - If loss > 5%: reduce resolution (1080p → 720p)
  - If loss > 15%: reduce framerate (30fps → 15fps)
  - If RTT spikes: increase keyframe interval
  - Recover when metrics improve
```

### Remote Control Protocol

```
Stream: dedicated bidirectional reliable stream
Priority: 1

Messages (postcard-encoded):
  KeyEvent {
    key: u32,          // Virtual key code
    modifiers: u8,     // Shift, Ctrl, Alt, Meta
    pressed: bool,
    timestamp: u64,
  }
  
  MouseClick {
    x: f32, y: f32,
    button: u8,
    pressed: bool,
    timestamp: u64,
  }
  
  MouseScroll {
    x: f32, y: f32,
    delta_x: f32,
    delta_y: f32,
    timestamp: u64,
  }
  
  ClipboardSync {
    format: ClipboardFormat,  // Text, Image, Files
    data: Vec<u8>,
  }
  
  ControlHandoff {
    to_peer: EndpointId,
    permissions: Permissions,  // FullControl, ViewOnly, etc.
  }
```

### Cursor Presence (Datagrams)

```
Datagram payload (< 50 bytes, postcard-encoded):
  CursorUpdate {
    peer_id: u16,      // Short session-local peer ID
    x: u16,            // Pixel position
    y: u16,
    visible: bool,
    click_animation: u8,  // 0 = none, 1-10 = click ripple frame
  }

Send rate: 60Hz when moving, 0Hz when still
Total bandwidth: ~3KB/s per peer
```

## Connection Lifecycle

### Peer Discovery
- **Primary**: Invite links with EndpointId + RelayUrl
- **LAN discovery**: iroh supports local network discovery (mDNS-based)
- **Persistent contacts**: Store EndpointId locally, reconnect via iroh DNS discovery

### Degradation Handling
```
Monitor via QUIC connection stats (RTT, loss, throughput):

RTT > 200ms:
  → Reduce screen FPS to 15
  → Increase keyframe interval to 5s

Loss > 5%:
  → Reduce screen resolution
  → Switch to more aggressive group expiry in moq-lite

Loss > 20%:
  → Screen sharing degrades gracefully (lower quality)
  → Audio stays (Opus handles packet loss well)
  → Control events still work (reliable stream)

Connection lost:
  → iroh attempts reconnection automatically
  → If direct path fails, falls back to relay
  → Resume session with 0-RTT if possible
  → UI shows "Reconnecting..." overlay
```

### Network Switch (WiFi → Ethernet)
With iroh/noq multipath:
1. New network interface detected
2. New QUIC path opened alongside existing path
3. Both paths active temporarily
4. Old path times out and is removed
5. No session interruption, no reconnection needed

### Symmetric NAT
- iroh's hole-punching fails (~10% of cases)
- Relay fallback kicks in automatically
- Performance: higher latency (relay round-trip) but still functional
- For video calls, relay latency is typically acceptable (50-150ms additional RTT)

## Phased Build Plan

### Phase 1: Proof of Concept (4-6 weeks)

**Goal**: Two peers share screen and cursor over iroh, no audio.

Deliverables:
- iroh Endpoint setup with invite link generation
- Screen capture via scap (30fps, 1080p)
- H.264 encoding via FFmpeg
- Screen frames sent over a raw QUIC reliable stream (skip moq-lite initially)
- Cursor position via datagrams
- Basic egui window showing remote screen
- Works on macOS first (easiest platform for capture + encode)

**Lines of code estimate**: ~3,000-4,000

This proves: iroh connectivity works, screen pipeline has acceptable latency, the architecture is viable.

### Phase 2: Protocol Layer (3-4 weeks)

**Goal**: Replace raw streams with moq-lite, add remote control.

Deliverables:
- Integrate moq-lite for screen track (priority, group model)
- Add keyboard/mouse input forwarding
- Add clipboard sync
- Quality adaptation based on network conditions
- Skip-static-frame optimization
- Basic control handoff UI

**Lines of code estimate**: ~2,000-3,000 additional

### Phase 3: Audio + Polish (4-6 weeks)

**Goal**: Full pair-programming experience.

Deliverables:
- Audio capture + Opus encoding via moq-lite audio track
- Multi-platform support (Windows, Linux)
- System tray / menu bar integration
- Session persistence (reconnect after network disruption)
- Recording (save session to file)
- Performance profiling and optimization

**Lines of code estimate**: ~3,000-5,000 additional

### Phase 4: Differentiation (ongoing)

- Terminal sharing (direct PTY forwarding, not screen capture)
- Editor integration (LSP-aware shared cursors)
- AI-assisted pairing features
- Multi-party sessions (3+ peers)
- Annotation/drawing tools
- Connection migration polish

## Risk Assessment

### 1. FFmpeg Dependency Complexity
**Risk**: FFmpeg is a massive C dependency with complex build configuration per platform.
**Mitigation**: Use `ffmpeg-next` with static linking. Pre-built binaries for CI. Consider vendoring. This is a solved problem (OBS, VLC, etc. all do this).
**Severity**: Medium. Annoying but not blocking.

### 2. iroh Pre-1.0 API Instability
**Risk**: iroh's API may change before and after 1.0, breaking your code.
**Mitigation**: Pin to specific versions. Wrap iroh behind your own transport trait. Monitor their changelog. The core API (Endpoint, connect, accept, streams) is unlikely to change fundamentally.
**Severity**: Low-Medium. API churn is annoying but manageable.

### 3. scap Maturity
**Risk**: scap is beta (0.1.0-beta.1). May have platform-specific bugs, missing features (no dirty-rect exposure), or performance issues.
**Mitigation**: Start on macOS where it's best supported. Have a fallback plan to use platform APIs directly if scap blocks you. Contribute fixes upstream.
**Severity**: Medium. Screen capture is foundational — if scap fails, you need a plan B.

### 4. Audio Echo Cancellation
**Risk**: No pure-Rust AEC exists. Users without headphones will have echo.
**Mitigation**: Phase 1-2 require headphones. Phase 3 integrates WebRTC audio processing via FFI. Most developer users wear headphones anyway.
**Severity**: Low for developer audience. High for general audience (but that's not your market).

### 5. QUIC/UDP Blocked Networks
**Risk**: Some corporate networks block or shape UDP traffic, breaking QUIC.
**Mitigation**: iroh's relay works over WebSockets (TCP), so connectivity is maintained. Performance degrades but it works. Could add a TCP fallback transport as last resort.
**Severity**: Low. Relay handles this case.

## Performance Targets

### MVP (Phase 1)

| Metric | Target | How to Measure |
|---|---|---|
| Screen latency (capture→display) | < 100ms | Stopwatch app on shared screen |
| Cursor latency | < 50ms | Timestamp comparison in datagrams |
| CPU usage (host, 1080p 30fps) | < 15% | Activity Monitor / top |
| Memory usage | < 200MB | RSS measurement |
| Connection setup time | < 3 seconds | Timestamp from click to first frame |
| Reconnection time | < 2 seconds | Network disconnect → first frame |

### Production (Phase 3)

| Metric | Target |
|---|---|
| Screen latency | < 60ms |
| Audio latency | < 40ms |
| CPU usage (1080p 30fps) | < 10% |
| Bandwidth (1080p 30fps) | < 5 Mbps |
| Connection setup | < 2 seconds |
| 5K support | Yes (hardware permitting) |
