# Research 1: Iroh + noq — The Connectivity Layer

## What is iroh?

Iroh is a Rust library by n0-computer for establishing direct peer-to-peer QUIC connections. It addresses peers by 32-byte ed25519 public keys (EndpointIds) rather than IP addresses, handles NAT traversal automatically via hole-punching, and falls back to relay servers when direct connections fail. It powers hundreds of thousands of devices in production today.

## What is noq?

noq ("number 0 QUIC") is n0's own general-purpose QUIC implementation, announced March 2026. It started as a Quinn fork but has diverged significantly. The key additions over Quinn:

- **QUIC Multipath** (full implementation of the IETF spec): Multiple network paths (relay, direct IPv4, direct IPv6) are first-class QUIC concepts with per-path congestion state. Before multipath, iroh faked this below the QUIC layer — Quinn thought it was talking to one IP while iroh shuffled packets across paths.
- **QUIC NAT Traversal (QNT)**: Hole-punching expressed directly as QUIC-level operations, making the congestion controller aware of it and improving loss detection. To their knowledge, they're the first to implement this in a production-grade way.
- **QUIC Address Discovery (QAD)**: Uses QUIC connections to learn public IP addresses, replacing STUN.
- **qlog support**: Extended logging for debugging, including multipath visualization.
- **WeakConnectionHandle**: For building connection managers without preventing connection cleanup.

noq is built as `noq` (async API on tokio) and `noq-proto` (sans-io protocol library), mirroring Quinn's architecture. It implements RFC 9000, RFC 9001, RFC 9002, RFC 8999, and the multipath/NAT traversal drafts.

## NAT Traversal — How It Works

### Connection Establishment Flow

1. Each endpoint generates an ed25519 keypair. The public key IS the EndpointId.
2. On startup, endpoints connect to the nearest relay server (n0 runs 4 public relays: 2 US, 1 Europe, 1 Asia) and register their EndpointId.
3. When peer A wants to connect to peer B, it needs: EndpointId, RelayUrl, and optionally direct addresses.
4. Initial connection routes through the relay (encrypted — relay can't read content).
5. Immediately, both peers begin hole-punching to establish a direct UDP path.
6. Once direct path succeeds, traffic migrates off the relay.

### QNT Specifics (v0.96+)

The QUIC-NAT-Traversal implementation:
- Client sends PATH_CHALLENGE to each received address on a new PathId
- Client sends REACH_OUT frames for each local candidate address
- Server receives REACH_OUT and sends PATH_CHALLENGE using existing connection IDs
- No complex "rounds" — when new addresses appear, previous rounds are aborted
- Requires multipath support (both are negotiated at connection setup)

### Success Rate

Iroh reports ~90% direct connection success (9 out of 10 connections go direct). The remaining ~10% fall back to relay. This is consistent with industry hole-punching success rates for UDP.

### Known Issues

- v0.96 has a regression where hole-punching is NOT re-triggered when network conditions change after initial connection. If you lose a direct path, you stay on relay until they fix it.
- One user report showed ~10KB still going through relay even after hole-punch succeeds (overhead from the multipath coordination).
- Dependency bloat: migrating from Quinn to iroh significantly increases compile times and binary size, even with feature flags disabled.

## Relay Server

### Architecture

- Relays are **stateless**: they route encrypted packets but store nothing.
- Protocol is based on Tailscale's DERP (Designated Encrypted Relay for Packets), revised.
- Connects via WebSockets over HTTPS.
- Provides QAD (QUIC Address Discovery) endpoints.
- v0.91 added challenge-based authentication to prevent replay attacks.

### Self-Hosting

- Need: a server with a public IP and DNS name pointing to it.
- ACME/Let's Encrypt TLS is built in.
- Config is a simple TOML file.
- The relay binary is open-source in the iroh repo (`iroh-relay` crate).
- n0 also offers managed "Iroh Services" dedicated relays.

### Cost Considerations

- Public n0 relays are free but rate-limited (intended for dev/testing).
- Relay is stateless and cheap to run — it's just packet forwarding.
- For a pair-programming app with ~90% direct connections, relay bandwidth is minimal.
- Main cost is the server itself, not bandwidth (a $5/mo VPS could handle many concurrent sessions).

## Minimal API for Your Use Case

```rust
use iroh::Endpoint;

const ALPN: &[u8] = b"pair-programming/v1";

// Peer A — create endpoint and listen
let endpoint = Endpoint::bind().await?;
let my_id = endpoint.endpoint_id();
// Share my_id + relay URL with peer B (via invite link, QR code, etc.)

// Accept incoming connection
let conn = endpoint.accept().await.unwrap().await?;
let (mut send, mut recv) = conn.open_bi().await?;

// Peer B — connect to Peer A
let endpoint = Endpoint::bind().await?;
let conn = endpoint.connect(peer_a_addr, ALPN).await?;
let (mut send, mut recv) = conn.open_bi().await?;

// Now you have a QUIC connection with:
// - End-to-end encryption
// - Automatic hole-punching
// - Relay fallback
// - Connection migration
// - Streams + datagrams
```

The API is essentially: create endpoint, connect by EndpointId, get QUIC streams. You don't need iroh-blobs, iroh-gossip, or iroh-docs — those are optional higher-level protocols.

## iroh + MoQ Integration

The moq-dev/moq repo has optional iroh support via a feature flag. When enabled:

```
iroh://<ENDPOINT_ID>: Connect via moq-lite over raw QUIC
h3+iroh://<ENDPOINT_ID>/path: Connect via WebTransport over HTTP/3 over iroh
```

This means you can publish MoQ broadcasts that are reachable via iroh's peer-to-peer connectivity, including NAT traversal. The moq-relay can run on iroh, and clients connect via endpoint IDs instead of IP addresses.

This is significant — it means the MoQ media layer and iroh's connectivity layer are already designed to work together.

## Assessment for Pair-Programming App

### Strengths
- NAT traversal is solved — ~90% direct, relay fallback for the rest
- Pure Rust, async, production-tested at scale
- EndpointId-based addressing is perfect for invite links
- Connection migration handles WiFi→Ethernet transitions
- Multipath means relay and direct paths coexist intelligently
- Already integrates with MoQ

### Risks
- Pre-1.0 (targeting September 2025 for 1.0, likely shipped by now — need to verify)
- Wire protocol may still change before 1.0
- Dependency size is significant
- QNT regression in v0.96 for network change re-triggering
- noq has diverged from Quinn — you're betting on n0's fork, not the community standard

### Recommendation

**Use iroh as the connectivity layer.** The NAT traversal alone justifies the dependency. You'd spend months building equivalent hole-punching on raw Quinn, and iroh gives it to you with a cleaner API. The iroh+MoQ integration is a bonus that de-risks the media layer decision.
