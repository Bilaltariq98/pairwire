//! iroh P2P connectivity layer for pairwire.

pub mod connection;
pub mod cursor;
pub mod datagram_router;
pub mod handshake;
pub mod invite;
pub mod moq_session;
pub mod quality;
pub mod transport;
pub mod wt_session;

pub use connection::PairwireConnection;
pub use cursor::{CursorReceiver, CursorReceiverHandle, CursorSender, CursorSenderHandle};
pub use datagram_router::{DatagramChannels, DatagramRouterHandle, TAG_CURSOR, TAG_GUEST_AUDIO};
pub use handshake::{NegotiatedSession, guest_handshake, host_handshake, local_handshake};
pub use invite::InviteLink;
pub use moq_session::{
    AudioStreamConfig, MoqSessionPublisher, MoqSessionPublisherHandle, MoqSessionSubscriber,
    MoqSessionSubscriberHandle, VideoStreamConfig,
};
pub use quality::{ConnectionHealth, QualityMonitor, QualityMonitorHandle, QualityTier};
pub use transport::Transport;

/// ALPN protocol identifier for pairwire connections.
pub const ALPN: &[u8] = b"pairwire/1";
