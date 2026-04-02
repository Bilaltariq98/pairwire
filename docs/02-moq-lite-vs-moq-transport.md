# Research 2: moq-lite vs moq-transport — Choosing the Right MoQ Layer

## The Two Implementations

There are now two distinct Rust MoQ implementations with different philosophies:

### 1. moq-dev/moq (Luke Curley / kixelated)
- Implements **moq-lite**, a forward-compatible subset of the IETF spec
- Focus: simplicity and deployability
- Layered architecture: moq-lite → hang → application
- Rust + TypeScript libraries
- Active development (commits through March 2026)
- Compatible with Cloudflare's MoQ CDN
- Luke Curley is the original creator of moq-rs, ex-Twitch engineer, now at Discord

### 2. cloudflare/moq-rs (Cloudflare, maintained by Mike English)
- Implements the **full IETF moq-transport draft-14**
- Focus: spec compliance and interoperability
- More complex: supports FETCH, namespaces, full signaling
- Targets the draft-07 branch for Cloudflare's production deployment
- Originally created by Luke Curley, now maintained separately

## Why moq-lite Exists

Luke Curley's own words from the moq-lite draft: "MoqTransport has become too complicated. There are too many messages, optional modes, and half-baked features. Too many hypotheses, too many potential use-cases, too many diametrically opposed opinions."

moq-lite is his opinionated simplification — fewer messages, no optional modes, designed to be deployable now rather than spec-complete later.

## moq-lite Architecture

```
┌─────────────────┐
│ Application     │  Your business logic
│                 │  authentication, non-media tracks, etc.
├─────────────────┤
│ hang            │  Media-specific encoding/streaming
│                 │  codecs, containers, catalog
├─────────────────┤
│ moq-lite        │  Generic pub/sub transport
│                 │  broadcasts, tracks, groups, frames
├─────────────────┤
│ WebTransport    │  Browser-compatible QUIC
│ QUIC            │  HTTP/3 handshake, multiplexing
└─────────────────┘
```

### moq-lite Layer (Generic Pub/Sub)

Core concepts:
- **Session**: A connection between client and server (no P2P within QUIC — out of scope)
- **Broadcast**: A discoverable collection of tracks from a single publisher
- **Track**: A named stream of data, split into groups. Has a priority field.
- **Group**: A sequential collection of frames, served by a dedicated QUIC stream. Closed on completion. Groups may arrive out of order due to congestion/prioritization.
- **Frame**: A chunk of data with known size. Should represent a single moment in time.

Key design: each Group gets its own QUIC stream. This means loss/delay in one group doesn't block others. The relay operates on rules in the moq-lite header without understanding media content.

### hang Layer (Media-Specific)

Built on top of moq-lite, provides:
- **Broadcast**: Collection of tracks with a catalog
- **Catalog**: Metadata describing available tracks, codec info (itself a live track)
- **Track**: Audio/video streams with typed metadata
- **Group**: Group of pictures (video) or sample collection (audio), independently decodable
- **Frame**: Timestamp + codec payload pair (implements WebCodecs spec)
- **cmaf module**: Import fMP4/CMAF files into hang broadcasts

Think of it as: hang is like HLS/DASH (media framing), moq-lite is like HTTP (generic transport).

### Rust API Example

```rust
use moq_lite::{Session, BroadcastProducer, Track};

// Establish session
let config = moq_native::ClientConfig::default();
let client = moq_native::Client::new(config);
let url = url::Url::parse("https://relay.moq.dev/anon")?;
let connection = client.connect(url).await?;
let mut session = moq_lite::Session::connect(connection).await?;

// Create broadcast with multiple tracks
let broadcast = moq_lite::BroadcastProducer::new();

// Screen frames track (high bandwidth, lower priority)
let screen_track = broadcast.create(Track {
    name: "screen".to_string(),
    priority: 2,  // Lower priority than audio/control
});

// Audio track (medium bandwidth, high priority)
let audio_track = broadcast.create(Track {
    name: "audio".to_string(),
    priority: 0,  // Highest priority
});

// Control events track (low bandwidth, high priority)
let control_track = broadcast.create(Track {
    name: "control".to_string(),
    priority: 1,
});

// Write a screen frame group
let group = screen_track.append();  // New QUIC stream
group.append(encoded_frame_bytes);

// Publish
session.publish("my-session", broadcast.consume());
```

## Priority and Congestion Handling

moq-lite's priority system:
- Each track has a numeric priority (lower = more important)
- Groups within a track are prioritized: newest groups are sent first
- Frames within a group are ordered (older frames first, since later frames depend on earlier ones)
- The relay makes forwarding decisions based on these headers WITHOUT understanding the media

Under congestion:
- Newer groups of high-priority tracks get bandwidth first
- Old groups of low-priority tracks may be dropped or delayed
- This is exactly what you want: audio stays smooth, old screen frames get dropped

For a pair-programming app:
- Priority 0: Audio
- Priority 1: Control (keyboard, mouse, clipboard)
- Priority 2: Cursor/presence (could also use datagrams)
- Priority 3: Screen frames

## The Relay

moq-relay is a clusterable server that:
- Connects multiple clients and servers
- Performs fan-out (1→N distribution)
- Operates on moq-lite header rules (doesn't inspect media)
- Supports JWT authentication
- Can be self-hosted or use Cloudflare's network

**Is a relay required?** For moq-lite as designed, yes — it's a client-server model, not P2P. However, with the iroh integration, you can run moq-lite over iroh's P2P connections, effectively making the relay optional for the direct connection case and using iroh's relay for NAT fallback.

**Compatibility:** moq-lite is forward-compatible with any moq-transport CDN, including Cloudflare's global MoQ relay network. This means you could use Cloudflare as your relay infrastructure.

## moq-transport (Full IETF Spec) Differences

The full IETF draft-17 adds:
- **FETCH**: Request historical data (not just live)
- **PUBLISH_NAMESPACE / SUBSCRIBE_NAMESPACE**: Hierarchical organization
- **More complex stream types**: SUBGROUP_HEADER, FETCH_HEADER
- **Fine-grained session termination codes**
- **More authentication/authorization hooks**

For a pair-programming app, you don't need any of these. You're streaming live data point-to-point. moq-lite is sufficient.

## Maturity Assessment

### moq-dev/moq
- ~880+ commits, active through March 2026
- Primary maintainer: kixelated (Luke Curley)
- Has OBS plugin, GStreamer plugin, web components
- hang.live is a working demo app
- libmoq v0.2.4 published (C bindings available)
- Known issues: some h264 parser bugs, Cloudflare Worker integration quirks
- Upgraded to Rust edition 2024

### cloudflare/moq-rs
- Maintained by Cloudflare (Mike English)
- Targets draft-14 (production) and tracking newer drafts
- More conservative, spec-focused
- Less opinionated about media layer

## Assessment for Pair-Programming App

### Use moq-lite + hang, not the full IETF spec

Reasons:
1. Simpler API, fewer concepts to manage
2. Priority system maps perfectly to collaboration traffic types
3. iroh integration already exists
4. Compatible with Cloudflare CDN if you need it later
5. The creator designed it specifically for the "build something now" use case
6. hang provides exactly the media framing you need

### Alternative: Custom Protocol on Raw QUIC

You could skip MoQ entirely and build custom streams on iroh/noq directly. This gives you maximum control but means reimplementing: track management, priority scheduling, group/frame semantics, relay forwarding logic.

**Recommendation:** Start with moq-lite for screen/audio tracks, but use raw QUIC streams/datagrams via iroh for control messages (keyboard, mouse, cursor presence). This hybrid approach gives you MoQ's media intelligence where it matters and raw speed where you need it.

---

## Decision Record (2026-04-02)

### hang adoption (P2.1.1) — required, deferred

The initial integration (P2.1, P3.1) used raw moq-lite tracks with a custom `ScreenFrameHeader`. hang was deliberately deferred to move fast on the screen+audio proof of concept. The plan is to adopt hang before Phase W (web viewer).

**What hang replaces:**
- `ScreenFrameHeader` (custom 8-byte timestamp + flags) → `hang::Frame` (timestamp + codec payload)
- Manual codec negotiation in `Handshake` → hang catalog track
- Manual track naming → hang broadcast with typed track names (`screen`, `audio/host`, `audio/guest_N`)

**What hang unlocks:**
- **Phase W**: hang frames are WebCodecs-native. `VideoDecoder` and `AudioDecoder` in the browser can consume them directly. No custom parser in the web client.
- **Multi-party audio**: Per-source track naming (`audio/host`, `audio/guest_1`, etc.) maps directly to hang's model. Guests subscribe to all `audio/*` tracks except their own — no extra protocol design.
- **Simulcast**: Publish multiple quality tracks in one catalog (`screen/1080p`, `screen/720p`, `audio/hifi`, `audio/asr`). Subscribers pick what they need. Foundation for on-demand quality selection and Whisper ASR audio track.

### moq-relay for multi-party (P5.3) — use the binary, don't build it

`moq-relay` is a deployable binary from the moq-dev/moq repo. Once hang adoption is complete, the host is already publishing standard moq-lite/hang tracks. Pointing guests at a moq-relay instance is a connection URL change, not a protocol change.

**Topology decision:**
- 1-5 peers: iroh P2P star (host is hub, encode once, broadcast channel fan-out). No relay needed.
- 6+ peers OR host has constrained upload: moq-relay SFU. Self-hosted or pairwire-operated.

Control, cursors, reactions, and input always go direct to host regardless of which topology is used — they are low-bandwidth and latency-sensitive.
