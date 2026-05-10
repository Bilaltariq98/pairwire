//! iroh Endpoint wrapper — singleton transport for a pairwire process.
//!
//! Create exactly one `Transport` per process. It owns the iroh `Endpoint`
//! and provides `host()` / `join()` entry points.

use std::time::Duration;

use iroh::{
    Endpoint,
    endpoint::{IdleTimeout, QuicTransportConfig, presets},
};
use pairwire_core::{Error, Result, SessionId};
use tracing::{info, instrument};

use crate::{ALPN, InviteLink, PairwireConnection};

/// How often to send QUIC PING frames to keep the connection alive.
///
/// Must be well below `TRANSPORT_MAX_IDLE_TIMEOUT` so that the peer receives
/// at least one keepalive before the idle timer fires.
const TRANSPORT_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(2);

/// Close the connection after this much silence from the remote.
///
/// The effective idle timeout is the **minimum** of both peers' values, so
/// setting 5 s on both sides means dead peers are detected in ≤ 5 s instead
/// of QUIC's 30-second default.
const TRANSPORT_MAX_IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// The pairwire transport layer. Owns the iroh `Endpoint` singleton.
pub struct Transport {
    endpoint: Endpoint,
}

impl Transport {
    /// Bind a new iroh endpoint using n0 defaults (relay + DNS address lookup).
    ///
    /// Configures a 2-second keepalive interval and 5-second max idle timeout
    /// so that dead peers are detected within ~5 seconds rather than the QUIC
    /// default of ~30 seconds.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint cannot be bound (e.g., port conflict).
    #[instrument(name = "transport_bind")]
    pub async fn bind() -> Result<Self> {
        let idle_timeout = IdleTimeout::try_from(TRANSPORT_MAX_IDLE_TIMEOUT)
            .map_err(|e| Error::Transport(format!("invalid idle timeout: {e}")))?;

        let transport_config = QuicTransportConfig::builder()
            .keep_alive_interval(TRANSPORT_KEEP_ALIVE_INTERVAL)
            .max_idle_timeout(Some(idle_timeout))
            .build();

        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![ALPN.to_vec()])
            .transport_config(transport_config)
            .bind()
            .await
            .map_err(|e| Error::Transport(format!("failed to bind endpoint: {e}")))?;

        info!(
            endpoint_id = %endpoint.id(),
            "iroh endpoint bound"
        );

        Ok(Self { endpoint })
    }

    /// Generate an invite link for this endpoint.
    ///
    /// Waits until the endpoint is online (has a home relay), then encodes
    /// the full `EndpointAddr` into the invite link.
    ///
    /// # Errors
    ///
    /// Currently infallible, but returns `Result` for forward compatibility.
    pub async fn invite_link(&self, session_id: SessionId) -> Result<InviteLink> {
        // Wait for relay assignment (usually < 1s on a healthy network)
        self.endpoint.online().await;
        let endpoint_addr = self.endpoint.addr();

        info!(
            endpoint_id = %endpoint_addr.id,
            relay_urls = ?endpoint_addr.relay_urls().collect::<Vec<_>>(),
            "invite link generated"
        );

        Ok(InviteLink::new(endpoint_addr, session_id))
    }

    /// Connect to a remote peer using an invite link.
    ///
    /// # Errors
    ///
    /// Returns an error if the QUIC connection cannot be established.
    #[instrument(name = "transport_connect", skip(self, invite), fields(peer = %invite.endpoint_id()))]
    pub async fn connect(&self, invite: &InviteLink) -> Result<PairwireConnection> {
        let conn = self
            .endpoint
            .connect(invite.endpoint_addr.clone(), ALPN)
            .await
            .map_err(|e| Error::Transport(format!("connect failed: {e}")))?;

        info!(
            remote_id = %conn.remote_id(),
            "connected to peer"
        );

        Ok(PairwireConnection::new(conn))
    }

    /// Accept the next incoming connection.
    ///
    /// Returns `None` if the endpoint is closed.
    ///
    /// # Errors
    ///
    /// Returns an error if the QUIC handshake fails on an incoming connection.
    #[instrument(name = "transport_accept", skip(self))]
    pub async fn accept(&self) -> Result<Option<PairwireConnection>> {
        let Some(incoming) = self.endpoint.accept().await else {
            return Ok(None);
        };

        let conn = incoming
            .await
            .map_err(|e| Error::Transport(format!("accept handshake failed: {e}")))?;

        info!(
            remote_id = %conn.remote_id(),
            "accepted connection from peer"
        );

        Ok(Some(PairwireConnection::new(conn)))
    }

    /// The local endpoint ID (public key).
    #[must_use]
    pub fn id(&self) -> iroh::EndpointId {
        self.endpoint.id()
    }

    /// Gracefully close the endpoint.
    pub async fn close(self) {
        self.endpoint.close().await;
    }
}
