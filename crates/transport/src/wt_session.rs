//! `web_transport_trait::Session` implementation over an iroh 0.97 connection.
//!
//! `web-transport-iroh` only supports iroh 0.96, so we implement the trait
//! directly. The mapping is straightforward: iroh's QUIC streams satisfy all
//! the required operations.

use std::fmt;

use bytes::Bytes;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use web_transport_trait as wt;

// ── Error ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct WtError(String);

impl fmt::Display for WtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "wt: {}", self.0)
    }
}

impl std::error::Error for WtError {}

impl wt::Error for WtError {
    fn session_error(&self) -> Option<(u32, String)> {
        None
    }
}

impl From<iroh::endpoint::ConnectionError> for WtError {
    fn from(e: iroh::endpoint::ConnectionError) -> Self {
        WtError(e.to_string())
    }
}

impl From<iroh::endpoint::WriteError> for WtError {
    fn from(e: iroh::endpoint::WriteError) -> Self {
        WtError(e.to_string())
    }
}

impl From<iroh::endpoint::ReadError> for WtError {
    fn from(e: iroh::endpoint::ReadError) -> Self {
        WtError(e.to_string())
    }
}

impl From<iroh::endpoint::ClosedStream> for WtError {
    fn from(e: iroh::endpoint::ClosedStream) -> Self {
        WtError(e.to_string())
    }
}

// ── SendStream ───────────────────────────────────────────────────────────────

pub struct WtSendStream(SendStream);

impl wt::SendStream for WtSendStream {
    type Error = WtError;

    async fn write(&mut self, buf: &[u8]) -> Result<usize, WtError> {
        self.0.write(buf).await.map_err(WtError::from)
    }

    fn set_priority(&mut self, order: u8) {
        // iroh uses i32 priority; map u8 → i32 (lower wt priority = higher iroh priority)
        let _ = self.0.set_priority(-i32::from(order));
    }

    fn finish(&mut self) -> Result<(), WtError> {
        self.0.finish().map_err(WtError::from)
    }

    fn reset(&mut self, code: u32) {
        let _ = self.0.reset(code.into());
    }

    async fn closed(&mut self) -> Result<(), WtError> {
        self.0.stopped().await.map_err(|e| WtError(e.to_string()))?;
        Ok(())
    }
}

// ── RecvStream ───────────────────────────────────────────────────────────────

pub struct WtRecvStream {
    inner: RecvStream,
}

impl WtRecvStream {
    fn new(inner: RecvStream) -> Self {
        Self { inner }
    }
}

impl wt::RecvStream for WtRecvStream {
    type Error = WtError;

    async fn read(&mut self, dst: &mut [u8]) -> Result<Option<usize>, WtError> {
        self.inner.read(dst).await.map_err(WtError::from)
    }

    fn stop(&mut self, code: u32) {
        let _ = self.inner.stop(code.into());
    }

    async fn closed(&mut self) -> Result<(), WtError> {
        // Drain remaining bytes until EOF
        let mut buf = [0u8; 1024];
        loop {
            match self.inner.read(&mut buf).await {
                Ok(None | Some(0)) => return Ok(()),
                Ok(Some(_)) => {} // keep draining
                Err(e) => return Err(WtError::from(e)),
            }
        }
    }
}

// ── Session ──────────────────────────────────────────────────────────────────

/// Wraps an iroh `Connection` as a `web_transport_trait::Session`.
///
/// Used to feed moq-lite's Client/Server without going through web-transport-iroh,
/// which is pinned to iroh 0.96.
#[derive(Clone)]
pub struct IrohWtSession {
    conn: Connection,
}

impl IrohWtSession {
    /// Create a session backed by an iroh connection.
    #[must_use]
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }
}

impl wt::Session for IrohWtSession {
    type SendStream = WtSendStream;
    type RecvStream = WtRecvStream;
    type Error = WtError;

    async fn accept_uni(&self) -> Result<WtRecvStream, WtError> {
        self.conn
            .accept_uni()
            .await
            .map(WtRecvStream::new)
            .map_err(WtError::from)
    }

    async fn accept_bi(&self) -> Result<(WtSendStream, WtRecvStream), WtError> {
        self.conn
            .accept_bi()
            .await
            .map(|(s, r)| (WtSendStream(s), WtRecvStream::new(r)))
            .map_err(WtError::from)
    }

    async fn open_bi(&self) -> Result<(WtSendStream, WtRecvStream), WtError> {
        self.conn
            .open_bi()
            .await
            .map(|(s, r)| (WtSendStream(s), WtRecvStream::new(r)))
            .map_err(WtError::from)
    }

    async fn open_uni(&self) -> Result<WtSendStream, WtError> {
        self.conn
            .open_uni()
            .await
            .map(WtSendStream)
            .map_err(WtError::from)
    }

    fn send_datagram(&self, payload: Bytes) -> Result<(), WtError> {
        self.conn
            .send_datagram(payload)
            .map_err(|e| WtError(e.to_string()))
    }

    async fn recv_datagram(&self) -> Result<Bytes, WtError> {
        self.conn
            .read_datagram()
            .await
            .map_err(|e| WtError(e.to_string()))
    }

    fn max_datagram_size(&self) -> usize {
        self.conn.max_datagram_size().unwrap_or(1200)
    }

    fn protocol(&self) -> Option<&str> {
        None // raw QUIC — moq-lite will negotiate via SETUP stream
    }

    fn close(&self, code: u32, reason: &str) {
        self.conn.close(code.into(), reason.as_bytes());
    }

    async fn closed(&self) -> WtError {
        self.conn.closed().await;
        WtError("connection closed".into())
    }
}
