# Research 9: Localhost Tunneling

## The Problem

Host runs `next dev` on localhost:3000. Guest can see the terminal output via screen share but can't open the app in their browser. They need the host's port tunneled to their own localhost — zero extra tools, zero config.

## 1. Architecture

```
GUEST MACHINE                         HOST MACHINE

Browser → localhost:3000              localhost:3000 ← next dev
    │                                       ▲
    ▼                                       │
TCP Listener (pairwire)              TCP Connector (pairwire)
    │                                       ▲
    ▼                                       │
QUIC bidi stream  ───────────────────►  accept bidi
 (one per TCP connection)               (tag 0x4C)
    │                                       │
    └── iroh P2P connection (encrypted) ────┘
```

Each TCP connection from the guest's browser opens a new QUIC bidirectional stream. Bytes are proxied in both directions. When either side closes, the stream closes.

### Why One QUIC Stream Per TCP Connection?

- **QUIC streams are cheap** — creation is essentially a frame in the QUIC packet (see `docs/03`).
- **No HOL blocking** — loss on one TCP connection doesn't block another.
- **Natural backpressure** — QUIC per-stream flow control maps directly to TCP flow control.
- **Simple cleanup** — stream close = TCP connection close, no multiplexing bookkeeping.

The alternative (one stream with connection IDs) reintroduces HOL blocking and requires a framing protocol on top of QUIC's already-framed streams. Not worth it.

### Stream Tagging

Following the pattern in `stream_router.rs` (screen = `0x53`), tunnel streams use tag `0x4C` ('L' for localhost). Since tunnels use **bidi** streams (unlike screen's uni streams), they're handled in a separate accept loop — no conflict with `UniStreamRouter`.

## 2. Protocol Types

All types go in `crates/protocol/src/tunnel.rs`.

### Negotiation (on the control stream)

```rust
/// Host advertises available ports during extended handshake.
TunnelOffer {
    ports: Vec<TunnelPort>,
}

TunnelPort {
    port: u16,
    label: Option<String>,   // "next dev", "api server"
    protocol: TunnelProtocol, // Http | Tcp
}

/// Guest requests a specific tunnel.
TunnelRequest {
    request_id: u32,       // correlates response
    host_port: u16,
    guest_port: u16,       // 0 = mirror host_port
    label: String,         // max 64 bytes
}

/// Host responds.
TunnelResponse {
    request_id: u32,
    result: TunnelResult,
}

TunnelResult {
    Accepted { tunnel_id: u16, guest_port: u16 },
    Denied { reason: String },
}

/// Either side tears down a tunnel.
TunnelClose { tunnel_id: u16, reason: String }
```

### Data Streams (per-TCP-connection bidi streams)

After the `0x4C` tag byte, the guest sends a length-prefixed postcard header:

```rust
/// Identifies which tunnel this TCP connection belongs to.
TunnelOpen {
    tunnel_id: u16,
    connection_id: u32,  // per-tunnel counter, for logging
}

/// Host responds after attempting localhost TCP connect.
TunnelOpenResult {
    Connected,
    Failed { reason: String },
}
```

After `Connected`, raw bytes flow bidirectionally — no further framing.

### Additions to Existing Types

```rust
// In ControlMessage enum:
TunnelOffer(TunnelOffer),
TunnelRequest(TunnelRequest),
TunnelResponse(TunnelResponse),
TunnelClose(TunnelClose),

// In Capability enum:
Tunnel,
```

## 3. Negotiation Flow

```
Host                              Guest
  │                                  │
  │◄──── Handshake (Capability::Tunnel) ──►│
  │                                  │
  │── TunnelOffer {3000, 8080} ────►│
  │                                  │
  │◄── TunnelRequest {3000} ────────│
  │                                  │
  │  (host checks: port allowed?     │
  │   approval needed? blocked?)     │
  │                                  │
  │── TunnelResponse {Accepted} ───►│
  │                                  │  Guest binds 127.0.0.1:3000
  │                                  │
  │  ── per TCP connection: ──       │  Browser opens localhost:3000
  │                                  │
  │◄── [bidi stream: 0x4C tag] ─────│  Guest opens QUIC bidi
  │◄── [TunnelOpen header] ─────────│
  │                                  │
  │  TCP connect 127.0.0.1:3000     │
  │                                  │
  │── [TunnelOpenResult: Connected] ►│
  │                                  │
  │◄═══ bidirectional byte proxy ══►│  Raw bytes both directions
  │                                  │
```

## 4. Security Model

### Defaults (security-first)

| Control | Default | Rationale |
|---------|---------|-----------|
| Tunnel ports | Explicit opt-in only | Host must configure `--tunnel 3000` or config |
| Database ports | **Blocked** | 3306, 5432, 6379, 27017, 9200, 2379, 5672, 11211 |
| Approval | Host must approve | Auto-accept is opt-in (`tunnel_auto_accept = true`) |
| Guest binding | `127.0.0.1` only | Never `0.0.0.0` — no LAN exposure |
| Reverse tunneling | Not supported | Guest can't expose their ports to host |
| Port scanning | Prevented | Guest can only request ports from `TunnelOffer` |
| Rate limiting | 5 requests / 10s | Exceeding auto-denies |
| Concurrent tunnels | Max 10 | Configurable |

### Error Message Policy

`TunnelOpenResult::Failed` always says "connection refused" — same message whether the port is blocked, not listening, or doesn't exist. Prevents information leakage about host's running services.

## 5. Implementation (~500 LoC)

### Guest Side: `TunnelListener` (~150 lines)

```
crates/transport/src/tunnel_listener.rs

TunnelListener {
    tunnel_id: u16,
    listener: TcpListener,  // bound to 127.0.0.1:guest_port
    conn: Arc<Connection>,
}

Accept loop:
  1. TCP accept
  2. Open QUIC bidi stream
  3. Write 0x4C tag
  4. send_message(TunnelOpen { tunnel_id, connection_id })
  5. recv_message() → TunnelOpenResult
  6. If Connected: spawn two copy tasks (TCP↔QUIC)
  7. If Failed: close TCP connection
```

### Host Side: `TunnelConnector` (~150 lines)

```
crates/transport/src/tunnel_connector.rs

TunnelConnector accepts bidi streams tagged 0x4C:
  1. recv_message() → TunnelOpen { tunnel_id, connection_id }
  2. Look up tunnel_id in registry → host_port
  3. TCP connect to 127.0.0.1:host_port
  4. If success: send_message(TunnelOpenResult::Connected), spawn copy tasks
  5. If failure: send_message(TunnelOpenResult::Failed), close stream
```

### Bidirectional Proxy (~50 lines)

```rust
async fn proxy_bidirectional(
    mut quic_send: SendStream,
    mut quic_recv: RecvStream,
    tcp: TcpStream,
) {
    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    tokio::select! {
        r = tokio::io::copy(&mut tcp_read, &mut quic_send) => { /* ... */ }
        r = tokio::io::copy(&mut quic_recv, &mut tcp_write) => { /* ... */ }
    }
    // Cleanup: finish QUIC stream, drop TCP halves
}
```

### Session Wiring (~100 lines)

In `run_host()`: spawn `TunnelConnector` accept loop after handshake.
In `run_join()`: send `TunnelRequest` for each `--tunnel` port, spawn `TunnelListener` per accepted tunnel.

## 6. Edge Cases

| Case | Behavior |
|------|----------|
| **Host port not listening** | TCP connect fails → `TunnelOpenResult::Failed` → guest TCP RST. Tunnel stays registered — next browser request retries. Handles dev server restarts. |
| **Guest port conflict** | Error with message: "Port 3000 in use. Use `--tunnel 3000:3001` to map differently." No auto-pick — predictable ports matter for dev workflows. |
| **Connection drops mid-transfer** | QUIC handles retransmission. If QUIC connection lost entirely (iroh reconnect), all TCP connections reset. Tunnels re-register on reconnect. |
| **Multiple TCP connections** | Browser opens 6+ connections per origin. Each gets own QUIC stream. Works naturally — QUIC handles thousands of streams. |
| **WebSocket upgrade** | Transparent. Tunnel operates at TCP byte level, doesn't parse HTTP. HMR/hot reload works out of the box. |
| **TLS passthrough** | Host runs HTTPS → TLS handshake is end-to-end between browser and host server. Tunnel forwards encrypted bytes unchanged. Double-encrypted (TLS inside QUIC) but harmless. |
| **Long-lived connections** | EventSource/SSE, WebSocket, HTTP/2 all work. QUIC streams have no timeout. |

## 7. Performance

### TCP-over-QUIC: No Meltdown

TCP-over-TCP (SSH tunnels) suffers from double retransmission amplification. TCP-over-QUIC avoids this entirely because QUIC uses UDP underneath. No cascading congestion backoff.

### Latency

| Phase | Cost |
|-------|------|
| QUIC bidi stream open | ~0ms (local operation, piggybacks on next packet) |
| TunnelOpen + TunnelOpenResult | 1 peer RTT (10-50ms LAN, 50-200ms internet) |
| Host TCP connect to localhost | <1ms |
| **Total per-TCP-connection setup** | **~1 peer RTT** |
| Steady-state per-byte | Negligible (no buffering beyond tokio I/O) |

### Throughput

Dev server traffic is typically <1 Mbps burst — barely registers against 3-5 Mbps screen sharing. For bulk transfers (database dumps), QUIC congestion window is shared with screen/audio. Set tunnel stream priority lower (5) than screen (3) so media quality isn't degraded.

## 8. Comparison with Alternatives

| | pairwire tunnel | ngrok | SSH -L | VS Code Ports | Tailscale |
|---|---|---|---|---|---|
| Extra setup | None (built-in) | Install + account | SSH server | VS Code only | Install + account |
| Architecture | P2P (direct) | Cloud relay | Depends | Cloud relay | Mesh VPN |
| Latency added | 0 (same QUIC conn) | 50-200ms | ~same | 50-200ms | ~same |
| Cost | Free | Freemium | Free | Free | Freemium |
| NAT traversal | Automatic (iroh) | Cloud server | Manual | Cloud | Automatic |
| Security | E2E encrypted, P2P | Through vendor infra | SSH tunnel | Through Microsoft | WireGuard |
| Works with pairing | Native | No | No | VS Code only | No |
| WebSocket/HMR | Transparent | Yes | Yes | Yes | Yes |

**pairwire advantage**: Zero friction — if you're already pairing, tunneling is one flag. No accounts, no servers, no extra tools. Lowest possible latency (reuses the same QUIC connection). Correct security model (session-scoped, host-approved, localhost-only).

## 9. Recommendations

1. **MVP**: Explicit port mapping via `--tunnel 3000,5432`. No auto-detect.
2. **Database ports blocked by default**. Must opt-in via config.
3. **One QUIC bidi stream per TCP connection** — simplest, QUIC handles multiplexing.
4. **Stream type tag `0x4C`** to distinguish from screen/moq-lite streams.
5. **Implement as ~500 LoC** across `tunnel_listener.rs`, `tunnel_connector.rs`, `protocol/tunnel.rs`.
6. **Host approval required by default**. Pre-approve via `--tunnel-allow` for trusted sessions.
7. **Generic error messages** — don't leak whether a port is blocked vs not listening.
8. **Tunnel priority = 5** (lower than screen=3, audio=0) so media isn't degraded by bulk transfers.
