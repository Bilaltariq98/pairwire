//! iroh Endpoint wrapper — singleton transport for a pairwire process.
//!
//! Create exactly one `Transport` per process. It owns the iroh `Endpoint`
//! and provides `host()` / `join()` entry points.

use iroh::{Endpoint, endpoint::presets};
use pairwire_core::{Error, Result, SessionId};
use tracing::{info, instrument};

use crate::{ALPN, InviteLink, PairwireConnection};

/// The pairwire transport layer. Owns the iroh `Endpoint` singleton.
pub struct Transport {
    endpoint: Endpoint,
}

impl Transport {
    /// Bind a new iroh endpoint using n0 defaults (relay + DNS address lookup).
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint cannot be bound (e.g., port conflict).
    #[instrument(name = "transport_bind")]
    pub async fn bind() -> Result<Self> {
        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![ALPN.to_vec()])
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
