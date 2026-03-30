# Research 3: QUIC Transport Primitives — Quinn / noq Deep Dive

## Quinn Overview

Quinn is the community-standard pure-Rust QUIC implementation. v0.11.9 is current, with 86M+ total downloads. It's built as:
- `quinn`: High-level async API on tokio
- `quinn-proto`: Sans-IO deterministic state machine (no runtime dependency)
- `quinn-udp`: Optimized UDP sockets with ECN support

noq (n0's fork) adds multipath and NAT traversal but maintains the same API shape. If you use iroh, you're using noq under the hood.

## Streams

### Types
- **Bidirectional**: Both sides can send/receive. Opened with `conn.open_bi()`.
- **Unidirectional**: One-way. Opened with `conn.open_uni()`.

### Overhead and Limits
- Streams are very cheap to create — QUIC was designed for many short-lived streams.
- No TCP-like connection establishment overhead per stream.
- Concurrent stream limits are negotiated during the QUIC handshake (configurable via `TransportConfig`).
- Default concurrent bidirectional streams: 100 (adjustable).
- Default concurrent unidirectional streams: 100 (adjustable).
- Flow control operates per-stream AND per-connection.

### API

```rust
// Open bidirectional stream
let (mut send, mut recv) = conn.open_bi().await?;
send.write_all(b"request data").await?;
send.finish()?;  // Signal end of send side
let response = recv.read_to_end(65536).await?;

// Open unidirectional stream
let mut send = conn.open_uni().await?;
send.write_all(b"one-way data").await?;
send.finish()?;

// Accept streams from peer
let (mut send, mut recv) = conn.accept_bi().await?;
let mut recv = conn.accept_uni().await?;
```

### Key Behaviors
- Streams are independent: loss on one doesn't block others (no HOL blocking)
- Data within a stream IS ordered and reliable
- Stream creation is essentially free (just a frame in the QUIC packet)
- Streams can be reset (aborted) with an error code

## Datagrams

QUIC datagrams (RFC 9221) provide unreliable, unordered delivery — like UDP but within the QUIC connection's encryption and congestion control.

### API

```rust
// Send datagram
conn.send_datagram(bytes::Bytes::from("cursor:100,200"))?;

// Receive datagram
let data = conn.read_datagram().await?;
```

### Characteristics
- **Max size**: Limited by the path MTU minus QUIC overhead. Typically ~1200 bytes for reliable delivery across all networks. Quinn provides `conn.max_datagram_size()`.
- **Unreliable**: No retransmission. If the packet is lost, the datagram is gone.
- **Unordered**: May arrive out of order.
- **Congestion-controlled**: Unlike raw UDP, datagrams still respect QUIC's congestion window. Under congestion, datagrams may be dropped before sending.
- **No fragmentation**: If your data exceeds max datagram size, you must fragment at the application layer.

### When Datagrams Are Dropped
- If the congestion window is full, `send_datagram` will return an error.
- In transit, normal packet loss applies but there's no retransmission.
- Perfect for ephemeral state where the next update supersedes the previous one.

## Priority and Scheduling

### Quinn's Priority Model
Quinn supports stream priorities. You can set priority on individual streams:

```rust
let mut send = conn.open_uni().await?;
send.set_priority(0)?;  // Higher priority (lower number = higher priority in Quinn)
```

The scheduler uses priorities to decide which stream gets bytes when the congestion window is limited. Higher priority streams are served first.

### noq Improvements
With multipath, noq also maintains per-path congestion state. This means:
- Relay path has its own congestion window
- Direct path has its own congestion window
- The scheduler can route high-priority data on the faster path

## Connection Migration

### How It Works
QUIC connections are identified by connection IDs, not IP:port tuples. When the underlying network changes (WiFi→Ethernet, IP change), QUIC can:
1. Detect the path change via incoming packets from a new address
2. Validate the new path (PATH_CHALLENGE/PATH_RESPONSE)
3. Continue the connection on the new path

### In Quinn
Quinn handles migration somewhat automatically — if packets arrive from a new address, it validates and migrates. The application doesn't need to drive it explicitly.

### In iroh/noq
Much more sophisticated with multipath:
- Multiple paths can be active simultaneously
- Traffic is scheduled across paths based on latency/throughput
- Network changes trigger path addition rather than migration
- The relay path serves as a persistent fallback

## 0-RTT and Fast Reconnect

### 0-RTT Resumption
QUIC supports sending data immediately on reconnection if the client has session state from a previous connection:

```rust
// Client stores session data after first connection
let session_data = conn.session_data().await;

// On reconnect, provide stored session data
let conn = endpoint.connect_with(config_with_0rtt, addr, server_name)?;
// Can immediately send data without waiting for handshake
```

### Security Tradeoffs
- 0-RTT data can be replayed by an attacker
- Only safe for idempotent requests
- Quinn lets you choose what to send in 0-RTT vs waiting for full handshake

### Practical Reconnection Speed
- Full handshake: 1 RTT (QUIC) vs 2-3 RTT (TCP+TLS)
- With 0-RTT: essentially 0 RTT for data transmission
- iroh adds overhead for relay coordination but the QUIC handshake itself is fast

## Performance Characteristics

### Throughput
- Quinn can saturate multi-gigabit links in benchmarks.
- CPU overhead is higher than raw UDP (encryption + congestion control) but lower than TCP+TLS (fewer copies, better batching).
- quinn-udp uses platform-specific optimizations: GSO (Generic Segmentation Offload) on Linux, similar batching on other platforms.

### Overhead
- Per-packet: ~20-30 bytes QUIC header + encryption overhead
- Per-stream: Negligible (just frame headers within packets)
- Memory: Proportional to number of active streams and buffer sizes

## Protocol Design for Pair-Programming App

### Stream/Datagram Layout

```
RELIABLE STREAMS (ordered, guaranteed delivery):
├── Control Stream (bidirectional, priority 0)
│   ├── Session establishment / auth
│   ├── Capability negotiation
│   ├── Permission changes (control handoff)
│   └── Session metadata updates
│
├── Keyboard Events Stream (unidirectional, priority 1)
│   └── Keydown/keyup with modifiers, timestamps
│
├── Mouse Events Stream (unidirectional, priority 1)
│   └── Click, scroll, drag events (NOT position — that's datagrams)
│
├── Clipboard Stream (unidirectional, priority 2)
│   └── Copy/paste data (can be large)
│
├── File Transfer Stream (unidirectional per file, priority 3)
│   └── Drag-and-drop file data
│
└── Screen Frames (via moq-lite tracks, priority 4)
    ├── Video track: keyframes + delta frames in groups
    └── Each group = one QUIC stream (moq-lite default)

UNRELIABLE DATAGRAMS (unordered, best-effort):
├── Cursor Position (< 50 bytes per update, 60Hz)
│   └── {x, y, peer_id, timestamp}
│
├── Presence/Status (< 100 bytes, 1-5Hz)
│   └── {typing, viewing_area, scroll_position}
│
├── Drawing/Annotation Points (< 100 bytes per point)
│   └── {x, y, color, size, stroke_id}
│
└── Diagnostic Heartbeat (< 50 bytes, 1Hz)
    └── {rtt, loss_rate, bitrate}

AUDIO (via moq-lite track, priority 0):
└── Audio track: 20ms Opus frames in groups
```

### Rationale

- **Control on reliable streams**: Session state must not be lost.
- **Input events on reliable streams**: A missed keypress is a bug. But use separate streams from control so they don't block each other.
- **Cursor on datagrams**: Position is ephemeral — the next update supersedes the previous one. Lost cursor positions don't matter. 60Hz * 50 bytes = 3KB/s — trivial.
- **Screen frames via moq-lite**: The group-per-QUIC-stream model means old frames don't block new ones. Priority scheduling means screen adapts to bandwidth while audio stays smooth.
- **Audio via moq-lite**: Gets the priority system and group independence. Each 20ms chunk is independently decodable.
- **Drawing on datagrams**: Similar to cursor — ephemeral visual state. Stroke completion can be confirmed on a reliable stream.

### Bandwidth Estimates (1080p, 30fps, two peers)

| Traffic Type | Bandwidth | Direction |
|---|---|---|
| Screen video (H.264) | 2-8 Mbps | Bidirectional |
| Audio (Opus) | 32-64 kbps | Bidirectional |
| Cursor/presence | 3-5 kbps | Bidirectional |
| Control events | < 1 kbps | Bidirectional |
| Keyboard/mouse | 1-5 kbps | One direction |
| **Total per peer** | **~2-9 Mbps** | |

This is well within QUIC's capabilities and typical home/office network capacity.
