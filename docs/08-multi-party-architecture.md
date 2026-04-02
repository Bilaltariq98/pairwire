# Research 8: Multi-Party Architecture

## Current State

pairwire is strictly 1:1. One iroh connection between host and guest. The host captures screen + audio, streams to one guest. The question: can we support 3-5 peers for mob programming, code reviews, and team sessions?

## 1. Can iroh Handle Multiple Connections?

**Yes.** An iroh `Endpoint` is a singleton per process (CLAUDE.md constraint), but it can accept multiple simultaneous connections. Each connection has independent:
- Congestion control (per-connection QUIC CC)
- Flow control windows
- Crypto state (TLS session)
- Stream multiplexing

The current `Transport::accept()` returns a single connection and is called once. For multi-party, this becomes an accept loop that spawns per-guest session tasks.

**What needs to change**: The session layer (`session.rs`), not the transport layer. Each guest gets its own `PairwireConnection`, its own moq-lite session, its own control stream. The capture/encode pipeline runs once and fans out to all guests.

## 2. Topology Analysis

### Star (Host-Centric) — Recommended for MVP

```
     Guest A ←── Host ──→ Guest B
                  │
                  └──→ Guest C
```

- Host is the hub. Encodes screen **once**, sends to all guests.
- Each guest has an independent QUIC connection to the host.
- Bandwidth cost: N × stream_bitrate on host's upload.
- Simplicity: each guest-host pair works exactly like the current 1:1 model.
- NAT traversal: only N hole-punches needed (each guest to host), not N².

### Mesh — Not Recommended for Screen

```
A ←→ B ←→ C ←→ Host
↕         ↕
└────→────┘
```

- O(N²) connections. 4 peers = 6 connections.
- Only one person's screen is shared — mesh is wasteful for the dominant data flow.
- NAT traversal must succeed for every pair. Probability of full mesh drops exponentially.
- **Only useful for audio** where everyone talks to everyone.

### Hybrid — Future Optimization

```
Screen/Terminal/Tunnels: Star (Host → all)
Audio: Mesh (everyone ↔ everyone, direct when possible)
Cursors/Reactions: Star relay through host (tiny bandwidth)
```

Best of both: star for heavy media (encode once), mesh for audio (low bandwidth, lower latency). But adds significant complexity. Save for Phase 2.

### SFU (Selective Forwarding Unit) — Scale-Out

```
Host ──→ SFU Server ──→ Guest A
                    ──→ Guest B
                    ──→ Guest C
```

Host uploads screen once to SFU, SFU re-broadcasts to all guests. Saves host bandwidth at the cost of an extra hop. The `moq-relay` crate from the moq-dev project is literally an SFU for moq-lite traffic.

**When needed**: Beyond 5 peers, or when host has constrained upload bandwidth.

## 3. Bandwidth Analysis

The real bottleneck is host upload bandwidth.

| Metric | Value |
|--------|-------|
| 1080p30 H.264 (ultrafast) | ~3-5 Mbps |
| 720p30 H.264 | ~1.5-2.5 Mbps |
| Opus audio (32kbps mono) | ~35 Kbps per peer |
| Cursor datagrams | <1 Kbps per peer |
| Reactions/drawing | <1 Kbps per peer |

### Host Upload by Guest Count

| Guests | Screen Upload | Audio Relay | Total Host Upload | Feasible? |
|--------|--------------|-------------|------------------|-----------|
| 1 (1:1) | 4 Mbps | 35 Kbps | ~4 Mbps | Trivially |
| 2 | 8 Mbps | 70 Kbps | ~8 Mbps | Yes, most connections |
| 3 | 12 Mbps | 105 Kbps | ~12 Mbps | Yes, decent upload |
| 4 | 16 Mbps | 140 Kbps | ~16 Mbps | Needs good upload |
| 5 | 20 Mbps | 175 Kbps | ~20 Mbps | Needs fast upload |
| 8 | 32 Mbps | 280 Kbps | ~32 Mbps | **Needs SFU** |

Audio relay is negligible. Screen is 99% of the cost.

**At 720p** (quality adaptation kicks in): 4 guests = ~10 Mbps. Much more feasible.

## 4. QUIC-Specific Challenges

### Slow Guest Isolation (Critical)

**Problem**: If Guest C has a slow connection, does it back-pressure the encoder for all guests?

**Answer**: No, if designed correctly. QUIC per-connection flow control is independent. The solution:

```
Encoder → broadcast::channel<EncodedPacket>
            ├─→ Guest A sender (fast) → sends all frames
            ├─→ Guest B sender (fast) → sends all frames
            └─→ Guest C sender (slow) → drops old, sends latest
```

`tokio::sync::broadcast` with a bounded buffer naturally handles this. Slow receivers get `RecvError::Lagged` and skip to the latest frame. Each guest's QUIC connection has its own congestion controller — backpressure on one connection doesn't affect others.

### Per-Guest Quality Adaptation

Each guest may have different network conditions:
- Guest A on fiber: full 1080p 30fps
- Guest B on cellular: needs 720p 15fps
- Guest C on relay: higher latency but functional

**Approach**: The host encodes at the highest quality. moq-lite's group-drop semantics handle per-guest adaptation — subscribers on slow connections automatically drop older groups, receiving fewer frames. The `QualityMonitor` can be per-connection, but the encoder doesn't need to change.

### NAT Traversal for N Peers

In star topology, each guest needs to hole-punch to the host independently. At 90% per-pair success rate:
- 2 guests: 81% both direct
- 4 guests: 66% all direct
- Failures fall back to iroh relay (encrypted, but adds latency)

This is manageable — only the failing guest pays the relay latency penalty. Others are unaffected.

### CPU Overhead

Per connection: TLS handshake, congestion state, stream management. At 5 connections, this is trivial. At 20+, crypto operations become noticeable. The 5-peer cap avoids this.

## 5. moq-lite for Multi-Party

### Can Multiple Subscribers Share a Broadcast?

**Yes — this is moq-lite's core design.** `BroadcastProducer` produces content, `BroadcastConsumer` subscribes. The publish/subscribe model is inherently one-to-many.

### Current Pattern (1:1)

```rust
let origin = OriginProducer::new();
let broadcast = BroadcastProducer::new();
let track = broadcast.create_track(Track { name: "screen", priority: 3 });
origin.publish_broadcast("pairwire/screen", broadcast.consume());
let _session = Server::new().with_publish(origin.consume()).accept(wt).await;
```

### Multi-Party Pattern

```rust
let origin = OriginProducer::new();
let broadcast = BroadcastProducer::new();
let track = broadcast.create_track(Track { name: "screen", priority: 3 });
origin.publish_broadcast("pairwire/screen", broadcast.consume());

// Per guest: each gets its own Server session but shares the same origin
for guest_conn in guests {
    let wt = IrohWtSession::new(guest_conn.raw_conn().clone());
    let _session = Server::new()
        .with_publish(origin.consume())  // new consumer handle, same origin
        .accept(wt).await;
}
```

The key insight: `origin.consume()` creates a new consumer handle each time. moq-lite internally handles the fan-out — when the publisher writes a frame, all connected subscribers receive it through their respective QUIC sessions.

### Audio in Multi-Party

Audio is bidirectional (everyone talks). Options:

1. **Host-side relay with per-source tracks** (recommended): Host publishes N audio tracks ("audio/host", "audio/guest_1", ...). Each guest subscribes to all tracks except their own. Guests handle volume locally. At 35 Kbps per track, even 5 tracks = 175 Kbps. Trivial.

2. **Host-side mixing**: Host mixes all audio into one track. Simpler for guests but loses per-peer mute/volume control.

3. **Mesh audio** (Phase 2): Guests establish direct connections for audio, bypassing the host. Better latency, but adds NAT traversal complexity.

**Recommend**: Start with option 1 (relay with per-source tracks). Move to mesh audio later if latency matters.

## 6. Protocol Changes Needed

### Handshake Additions

```rust
Handshake {
    // existing fields...
    peer_id: u8,           // assigned by host (0 = host)
    display_name: String,  // from config
    cursor_color: [u8; 3], // distinct per peer
}
```

### New Control Messages

```rust
PeerJoined { peer_id: u8, name: String, color: [u8; 3] }
PeerLeft { peer_id: u8 }
PeerList { peers: Vec<PeerInfo> }  // sent to new guests on join
```

### Existing Types (Already Multi-Party Ready)

- `CursorUpdate` already has `peer_id: u16`
- `ControlHandoff` already has `to_peer: u16`
- `Reaction` will have `peer_id` when implemented

The protocol was designed with multi-party in mind.

## 7. Implementation Phases

### Phase 1 (P5.1): Star Topology

```
Accept loop → spawn per-guest handler
Encoder → broadcast channel → per-guest ScreenSender
Audio → per-source tracks relayed through host
Cursors → host aggregates, re-broadcasts
Control → one guest drives at a time
```

Changes: ~300 lines in `session.rs` + broadcast channel wiring. The transport, capture, encode, and decode layers don't change.

### Phase 2 (P5.2): Mesh Audio

Guests discover each other via host. Each pair establishes a direct iroh connection for audio. Falls back to host relay if direct path fails.

### Phase 3 (P5.3): Optional SFU

Deploy a `moq-relay` server. Host publishes to it instead of directly to guests. Guests subscribe from relay. Only needed for 6+ peers.

## 8. Practical Limits

**Recommend: Cap at 5 peers (1 host + 4 guests).**

This covers:
- **Pair programming** (2 people) — the core use case
- **Code review** (3 people — author + 2 reviewers)
- **Mob programming** (4-5 people)

At 5 peers, host needs ~20 Mbps upload at 1080p. With quality adaptation to 720p, this drops to ~12 Mbps — feasible on most broadband.

Beyond 5, SFU becomes necessary. The architecture supports this as an additive change (deploy relay, point guests at it) — not a rewrite.

## 9. Privacy

### Tuple's Approach

> "With 3 or more participants, media may be routed through our servers."

Tuple does this because NAT traversal for N peers in mesh is unreliable and their SFU simplifies the architecture at scale.

### pairwire's Approach

**All connections are end-to-end encrypted (QUIC TLS 1.3).** Even when traffic goes through iroh's relay, the relay forwards encrypted packets — it cannot read the content.

| Mode | Privacy Level |
|------|--------------|
| Direct P2P (hole-punch succeeds) | Full — no third party sees any data |
| Via iroh relay (hole-punch fails) | Encrypted — relay forwards opaque packets |
| Via SFU (future, 6+ peers) | Self-hosted = full control. Cloud = trust the SFU operator |

For the 2-5 peer star topology, privacy is preserved regardless of NAT traversal outcome. The host is the only "relay" and they're a session participant.

## 10. Recommendations

1. **Start with star topology** — simplest, covers 95% of use cases, minimal code changes.
2. **Encode once, fan out via `broadcast::channel`** — slow guests drop frames independently.
3. **Default cap at 5 peers** — configurable higher if host has bandwidth.
4. **Per-source audio tracks** — no server-side mixing needed at this scale.
5. **Mesh audio is Phase 2** — only needed if relay latency becomes noticeable.
6. **SFU is Phase 3** — `moq-relay` is the natural fit when scale demands it.
7. **Show bandwidth usage in session bar** — so host knows their limits.
8. **Protocol types are already multi-party ready** — `peer_id` fields exist. Main work is session orchestration.
