//! iroh P2P connectivity layer for pairwire.

pub mod connection;
pub mod cursor;
pub mod handshake;
pub mod invite;
pub mod moq_audio;
pub mod moq_screen;
pub mod quality;
pub mod screen;
pub mod stream_router;
pub mod transport;
pub mod wt_session;

pub use connection::PairwireConnection;
pub use cursor::{CursorReceiver, CursorReceiverHandle, CursorSender, CursorSenderHandle};
pub use handshake::{NegotiatedSession, guest_handshake, host_handshake, local_handshake};
pub use invite::InviteLink;
pub use moq_audio::{
    MoqAudioPublisher, MoqAudioPublisherHandle, MoqAudioSubscriber, MoqAudioSubscriberHandle,
};
pub use moq_screen::{
    MoqScreenPublisher, MoqScreenPublisherHandle, MoqScreenSubscriber, MoqScreenSubscriberHandle,
};
pub use quality::{ConnectionHealth, QualityMonitor, QualityMonitorHandle, QualityTier};
pub use screen::{ReceiverHandle, ScreenReceiver, ScreenSender, SenderHandle};
pub use stream_router::{NonScreenReceiver, UniStreamRouter};
pub use transport::Transport;

/// ALPN protocol identifier for pairwire connections.
pub const ALPN: &[u8] = b"pairwire/1";
