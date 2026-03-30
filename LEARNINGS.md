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
