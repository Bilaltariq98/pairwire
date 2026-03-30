//! iroh P2P connectivity layer for pairwire.

pub mod connection;
pub mod cursor;
pub mod invite;
pub mod screen;
pub mod transport;

pub use connection::PairwireConnection;
pub use cursor::{CursorReceiver, CursorReceiverHandle, CursorSender, CursorSenderHandle};
pub use invite::InviteLink;
pub use screen::{ReceiverHandle, ScreenReceiver, ScreenSender, SenderHandle};
pub use transport::Transport;

/// ALPN protocol identifier for pairwire connections.
pub const ALPN: &[u8] = b"pairwire/1";
