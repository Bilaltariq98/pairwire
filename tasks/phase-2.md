# Phase 2: Protocol Layer + Remote Control

> Goal: Replace raw streams with moq-lite, add keyboard/mouse input, clipboard sync, quality adaptation.
> Estimated effort: 3–4 weeks

## Tasks

### P2.1 — moq-lite Integration (Screen Track)
- **Status**: ✅ Complete
- **Crate**: `crates/transport`
- **Key deliverables**: moq-lite session over iroh connection; screen published as a track (priority 3); groups map to keyframe intervals; subscriber feeds decoder
- **Depends on**: P1.10

### P2.2 — Session Handshake & Capability Negotiation
- **Status**: ✅ Complete
- **Crate**: `crates/transport`, `crates/app`
- **Key deliverables**: `HandshakeSession` — exchanges `Handshake` on control stream immediately after connect; version check; codec/capability intersection; `NegotiatedSession` result type
- **Depends on**: P1.2, P0.3

### P2.3 — Keyboard & Mouse Input Forwarding
- **Status**: ✅ Complete
- **Crate**: `crates/input`
- **Key deliverables**: `InputSender` (guest: capture events → send on QUIC stream), `InputReceiver` (host: receive → inject via CGEvent on macOS); permission gate via `ControlHandoff`
- **Depends on**: P2.2

### P2.4 — Clipboard Sync
- **Status**: ✅ Complete
- **Crate**: `crates/input`
- **Key deliverables**: Clipboard monitor, `ClipboardSync` message exchange, 1MB size limit, configurable auto/manual sync
- **Depends on**: P2.2

### P2.5 — Quality Adaptation
- **Status**: ✅ Complete
- **Crate**: `crates/transport`, `crates/capture`
- **Key deliverables**: QUIC stats monitor (RTT, loss); adaptation rules (loss >5% → 720p, >15% → 15fps, RTT >200ms → lower fps + longer keyframe interval); `Heartbeat` datagram exchange
- **Depends on**: P2.1

### P2.6 — Skip Static Frames Optimization
- **Status**: ✅ Complete
- **Crate**: `crates/capture`
- **Key deliverables**: Block-level hash comparison; skip encode+send when static; "still alive" keepalive after N static frames; immediate resume on change
- **Depends on**: P1.4
- **Note**: Partially implemented in P1.4 (static frame hash detection). This task completes the network-level skip and keepalive.
