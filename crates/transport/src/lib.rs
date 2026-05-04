//! iroh P2P connectivity layer for pairwire.

pub mod connection;
pub mod control_channel;
pub mod cursor;
pub mod datagram_router;
pub mod drawing;
pub mod handshake;
pub mod invite;
pub mod moq_session;
pub mod quality;
pub mod transport;
pub mod wt_session;

#[cfg(test)]
pub(crate) mod test_utils;

pub use connection::PairwireConnection;
pub use control_channel::{ControlChannel, ControlChannelHandle};
pub use cursor::{CursorReceiver, CursorReceiverHandle, CursorSender, CursorSenderHandle};
pub use datagram_router::{
    DatagramChannels, DatagramRouterHandle, TAG_CURSOR, TAG_DRAW_POINT, TAG_GUEST_AUDIO,
    TAG_REACTION,
};
pub use drawing::{
    DrawingEvent, DrawingReceiver, DrawingReceiverHandle, DrawingSender, DrawingSenderHandle,
    STROKES_PER_PEER_CAP, TimedStroke,
};
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
