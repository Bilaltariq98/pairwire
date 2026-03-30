//! iroh P2P connectivity layer for pairwire.
//!
//! Wraps iroh's `Endpoint` to provide:
//! - Invite link generation and parsing
//! - Typed QUIC stream helpers (control, input, screen)
//! - Cursor datagram send/receive
//! - Connection lifecycle management
//!
//! The iroh `Endpoint` is a singleton — create exactly one per process.
//! All network I/O is async on the tokio runtime.

pub mod connection;
pub mod invite;
pub mod transport;

pub use connection::PairwireConnection;
pub use invite::InviteLink;
pub use transport::Transport;

/// ALPN protocol identifier for pairwire connections.
pub const ALPN: &[u8] = b"pairwire/1";
