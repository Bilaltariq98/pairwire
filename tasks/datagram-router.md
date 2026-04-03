# Task: Build DatagramRouter for tagged datagram dispatch

## Context

moq-lite owns all QUIC streams (uni + bidi) on the connection. It NEVER reads datagrams — verified in moq-lite 0.15.5 source. Datagrams are our only safe side-channel for low-latency unreliable data (cursor, guest audio, future reactions/drawing).

Currently `CursorReceiver` calls `conn.read_datagram()` directly. If we add a second reader (guest audio), they steal each other's datagrams non-deterministically. The fix: ONE reader that dispatches by tag byte.

## Architecture

```
QUIC Connection
  ├── Streams (owned by moq-lite — DO NOT TOUCH)
  │   └── video0, audio0, catalog.json via hang framing
  │
  └── Datagrams (ours — moq-lite never reads these)
      └── DatagramRouter (single reader)
          ├── Tag 0x43 'C' → cursor_tx (CursorUpdate)
          ├── Tag 0x47 'G' → guest_audio_tx (OpusPacket)
          ├── Tag 0x52 'R' → reaction_tx (future)
          └── Tag 0x44 'D' → draw_point_tx (future)
```

Wire format: `[1 byte tag] [payload bytes...]`

## Files to create/modify

### 1. CREATE `crates/transport/src/datagram_router.rs`

```rust
//! Single-reader datagram dispatcher with tag-byte routing.

use bytes::Bytes;
use iroh::endpoint::Connection;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Tag bytes for datagram types. First byte of every datagram.
pub const TAG_CURSOR: u8 = 0x43;       // 'C'
pub const TAG_GUEST_AUDIO: u8 = 0x47;  // 'G'
// Future: TAG_REACTION = 0x52, TAG_DRAW = 0x44

/// Channels that the router dispatches to.
pub struct DatagramChannels {
    pub cursor_tx: mpsc::Sender<Bytes>,
    pub guest_audio_tx: Option<mpsc::Sender<Bytes>>,
    // Add more Option<mpsc::Sender<Bytes>> as needed
}

/// Handle to stop the router task.
pub struct DatagramRouterHandle {
    join: tokio::task::JoinHandle<()>,
}

impl DatagramRouterHandle {
    pub async fn stop(self) {
        self.join.abort();
        let _ = self.join.await;
    }
}

/// Start the single datagram reader. Returns a handle.
/// All received datagrams are routed by their first byte tag.
pub fn start(conn: Connection, channels: DatagramChannels) -> DatagramRouterHandle {
    let join = tokio::spawn(async move {
        info!("datagram router started");
        loop {
            let data = match conn.read_datagram().await {
                Ok(d) => d,
                Err(e) => {
                    info!(error = %e, "datagram router stopped");
                    break;
                }
            };

            if data.is_empty() {
                continue;
            }

            let tag = data[0];
            let payload = data.slice(1..);

            match tag {
                TAG_CURSOR => {
                    let _ = channels.cursor_tx.try_send(payload);
                }
                TAG_GUEST_AUDIO => {
                    if let Some(ref tx) = channels.guest_audio_tx {
                        let _ = tx.try_send(payload);
                    }
                }
                _ => {
                    debug!(tag, len = data.len(), "unknown datagram tag, dropping");
                }
            }
        }
    });

    DatagramRouterHandle { join }
}

/// Send a tagged datagram. Prepends the tag byte to the payload.
pub fn send_tagged(conn: &Connection, tag: u8, payload: &[u8]) -> Result<(), String> {
    let mut buf = Vec::with_capacity(1 + payload.len());
    buf.push(tag);
    buf.extend_from_slice(payload);
    conn.send_datagram(Bytes::from(buf))
        .map_err(|e| e.to_string())
}
```

### 2. MODIFY `crates/transport/src/cursor.rs`

Replace direct `conn.send_cursor_datagram()` / `conn.recv_cursor_datagram()` with:

**CursorSender**: Use `datagram_router::send_tagged(conn, TAG_CURSOR, &postcard_bytes)` instead of `conn.send_datagram(bytes)`.

**CursorReceiver**: DELETE entirely. Replace with a consumer of the router's `cursor_tx` channel.

New `CursorReceiver`:
```rust
pub struct CursorReceiver;

impl CursorReceiver {
    /// Spawn a task that reads from the router's cursor channel
    /// and publishes to a watch::Sender.
    pub fn spawn(
        mut cursor_rx: mpsc::Receiver<Bytes>,
    ) -> (CursorReceiverHandle, watch::Receiver<Option<CursorUpdate>>) {
        let (tx, rx) = watch::channel(None);
        let handle = tokio::spawn(async move {
            while let Some(payload) = cursor_rx.recv().await {
                match postcard::from_bytes::<CursorUpdate>(&payload) {
                    Ok(update) => { let _ = tx.send(Some(update)); }
                    Err(e) => { debug!(error = %e, "invalid cursor datagram"); }
                }
            }
        });
        (CursorReceiverHandle { handle }, rx)
    }
}
```

### 3. MODIFY `crates/transport/src/connection.rs`

Remove `send_cursor_datagram()` and `recv_cursor_datagram()` methods. They're replaced by the router.

Keep `send_datagram()` as a raw escape hatch if needed, but mark it `#[allow(dead_code)]`.

### 4. MODIFY `crates/transport/src/lib.rs`

Add: `pub mod datagram_router;`
Export: `pub use datagram_router::{DatagramChannels, DatagramRouterHandle, TAG_CURSOR, TAG_GUEST_AUDIO};`

### 5. ADD guest audio send/receive

**Guest side** (in `moq_session.rs` guest_session or `session.rs` run_join):
```rust
// Send guest audio as tagged datagrams
tokio::spawn(async move {
    while let Some(pkt) = guest_audio_rx.recv().await {
        let _ = datagram_router::send_tagged(
            conn.raw_conn(), TAG_GUEST_AUDIO, &pkt.data
        );
    }
});
```

**Host side** (in `session.rs` run_host):
```rust
// Router dispatches TAG_GUEST_AUDIO datagrams to guest_audio_rx
// → feed into AudioDecodeThread → AudioPlaybackThread
```

### 6. MODIFY `crates/app/src/session.rs`

**run_host()**:
```rust
// After handshake, before moq session:
let (cursor_chan_tx, cursor_chan_rx) = mpsc::channel(64);
let (guest_audio_chan_tx, guest_audio_chan_rx) = mpsc::channel(64);

let router = datagram_router::start(
    conn.raw_conn().clone(),
    DatagramChannels {
        cursor_tx: cursor_chan_tx,
        guest_audio_tx: Some(guest_audio_chan_tx),
    },
);

// CursorReceiver reads from cursor_chan_rx (not from connection directly)
let (cursor_handle, _cursor_watch) = CursorReceiver::spawn(cursor_chan_rx);

// Guest audio: guest_audio_chan_rx → wrap in OpusPacket → AudioDecodeThread
tokio::spawn(async move {
    while let Some(data) = guest_audio_chan_rx.recv().await {
        let pkt = OpusPacket { data, timestamp_us: 0, duration_ms: 20 };
        let _ = guest_audio_decode_tx.send(pkt).await;
    }
});
```

**run_join()**:
```rust
// Router with no guest_audio_tx (guest doesn't receive its own audio)
let (cursor_chan_tx, cursor_chan_rx) = mpsc::channel(64);
let router = datagram_router::start(
    conn.raw_conn().clone(),
    DatagramChannels {
        cursor_tx: cursor_chan_tx,
        guest_audio_tx: None,
    },
);

// CursorReceiver from router channel (not connection)
// CursorSender uses send_tagged(conn, TAG_CURSOR, ...)

// Guest audio SENDER:
tokio::spawn(async move {
    while let Some(pkt) = guest_opus_rx.recv().await {
        let _ = datagram_router::send_tagged(
            conn.raw_conn(), TAG_GUEST_AUDIO, &pkt.data
        );
    }
});
```

## Constraints

- **Do NOT change moq_session.rs publish/subscribe pattern** — it works. Only add the datagram router alongside it.
- **Do NOT add `with_consume()` to the Server or `with_publish()` to the Client** — bidirectional moq-lite is broken for our use case.
- moq-lite NEVER calls `read_datagram()` — confirmed in source. Our router is the sole reader.
- Opus packets at 32kbps ≈ 80 bytes per 20ms. With 1-byte tag = 81 bytes. Well under 1200-byte QUIC datagram MTU.
- Cursor updates ≈ 10 bytes + 1 tag = 11 bytes. Also fine.

## Validation

After implementation:
1. `just all` passes
2. `pairwire host` + `pairwire join` — screen appears in viewer (no "Connecting..." stuck)
3. Host audio plays on guest speakers
4. Guest audio plays on host speakers (NEW)
5. No "cursor datagram recv error" warnings in logs

## Test order

1. Build DatagramRouter + migrate CursorSender/CursorReceiver
2. Test: screen + host audio still works (regression check)
3. Add guest audio send/receive via tagged datagrams
4. Test: bidirectional audio works
