# CONVENTIONS.md — Established Patterns

> Update this file when you establish a new pattern or discover a better approach.
> This is the ground truth for how code is written in this repo.

---

## Error Handling

**Library crates** use `pairwire_core::Error` (thiserror) and `pairwire_core::Result<T>`.

```rust
// In a library crate
use pairwire_core::{Error, Result};

fn parse_invite(s: &str) -> Result<InviteLink> {
    let inner = s.strip_prefix("pairwire://")
        .ok_or_else(|| Error::Protocol("missing scheme".into()))?;
    // ...
}
```

**The `app` crate** uses `anyhow::Result` for ergonomic error propagation in `main`.

```rust
// In crates/app/src/main.rs
use anyhow::Result;
fn main() -> Result<()> { ... }
```

Never use `unwrap()` outside of `#[cfg(test)]` blocks. Use `?` or explicit `map_err`.

---

## Async & Threading

- **tokio runtime** is used for all network I/O and async coordination.
- **Capture, encode, decode, audio** run on dedicated OS threads (`std::thread::spawn`), not tokio tasks. These threads own non-`Send` resources (FFmpeg contexts, scap capturer).
- Bridge OS threads to tokio with `tokio::sync::mpsc` channels.

```rust
// Correct: OS thread → tokio channel
let (tx, rx) = tokio::sync::mpsc::channel::<RawFrame>(4);
std::thread::spawn(move || {
    loop {
        let frame = capturer.next_frame();
        let _ = tx.blocking_send(frame); // blocking_send from non-async context
    }
});
```

---

## Channel Patterns

| Use case | Channel type | Capacity |
|---|---|---|
| Capture → encode | `tokio::sync::mpsc` | 4 (drop oldest on overflow) |
| Encode → network | `tokio::sync::mpsc` | 4 |
| Network → decode | `tokio::sync::mpsc` | 8 |
| Decode → UI (latest frame) | `arc_swap` or `AtomicCell` | 1 (single-slot) |
| Cursor datagrams | `crossbeam_channel::bounded` | 1 (low-latency) |
| Test helpers | `std::sync::mpsc::sync_channel` | varies |

Keep `core` free of `tokio` — use `std::sync::mpsc` in `core::test_utils`.

---

## Serialization

All wire protocol types live in `crates/protocol`. Serialized with `postcard`.

Rules:
1. **Never reorder fields** in a serialized type — postcard is not self-describing.
2. **Only append** new fields to existing types.
3. **Version all protocol types** via the `PROTOCOL_VERSION` constant.
4. Every protocol type must have a `#[cfg(test)]` round-trip test.
5. Size-constrained types (datagrams) must have a size assertion test.

```rust
// Required test pattern for every protocol type
#[test]
fn my_type_round_trip() {
    let val = MyType { ... };
    let bytes = postcard::to_allocvec(&val).unwrap();
    assert_eq!(val, postcard::from_bytes(&bytes).unwrap());
}
```

---

## Logging

Use `tracing` with structured fields. Never `println!` in library crates.

```rust
// Good
tracing::info!(peer_id = %peer.short(), rtt_us = stats.rtt, "peer connected");
tracing::warn!(loss_per_mille = loss, "high packet loss, reducing quality");

// Bad
println!("peer connected: {}", peer_id);
```

Log levels:
- `error!` — unrecoverable failures that affect the session
- `warn!` — degraded operation (quality reduction, relay fallback)
- `info!` — session lifecycle events (connect, disconnect, handshake)
- `debug!` — per-frame stats, channel activity (high volume, off by default)
- `trace!` — raw bytes, datagram contents (very high volume)

---

## Testing

- Unit tests: `#[cfg(test)] mod tests` inside the source file.
- Integration tests: `tests/` at workspace root.
- Hardware-dependent tests (real screen capture, real audio): gate with `#[ignore]`.
- Use `FakeFrameSource` and `FakeAudioSource` from `pairwire_core::test_utils` instead of real hardware.
- Use `assert_round_trip` from `pairwire_protocol::test_utils` for all protocol types.

```rust
// Gating a hardware test
#[test]
#[ignore = "requires display hardware"]
fn real_capture_test() { ... }
```

---

## Naming

- Crate names: hyphenated (`pairwire-core`, `pairwire-transport`)
- Module names: snake_case (`test_utils`, `frame_header`)
- Types: PascalCase (`RawFrame`, `EncodedPacket`, `CursorUpdate`)
- Constants: SCREAMING_SNAKE_CASE (`PROTOCOL_VERSION`, `MAX_DATAGRAM_SIZE`)
- Async functions that open connections: `connect_*`, `accept_*`
- Thread-spawning functions: `start() -> Receiver<T>` pattern

---

## Unsafe Code

Every `unsafe` block requires a `// SAFETY:` comment explaining the invariant being upheld.

```rust
// SAFETY: The pointer is valid for the lifetime of `buf` and we have exclusive access.
unsafe { std::ptr::copy_nonoverlapping(src, dst, len) };
```

Avoid `unsafe` unless required for FFI (FFmpeg, platform APIs).

---

## Feature Flags

- `test-support`: exposes `test_utils` modules in library crates for use in integration tests.
  Add to `Cargo.toml` of any crate that has test helpers.
- Platform-specific capture backends will be feature-gated (e.g., `capture-screencapturekit`).
