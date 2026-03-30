//! Terminal sharing via PTY forwarding over QUIC streams.
//!
//! This is pairwire's wedge feature: pixel-perfect, near-zero latency,
//! minimal bandwidth — no screen capture or video encoding required.
//!
//! Architecture:
//! - Host: spawns a PTY, forwards PTY output to a QUIC stream
//! - Guest: renders PTY output in a terminal emulator, sends keystrokes back
//!
//! Unlike screen sharing, terminal sharing works over relay connections
//! with acceptable latency even at 200ms RTT.
//!
//! Implemented in Phase 4 (P4.x).
