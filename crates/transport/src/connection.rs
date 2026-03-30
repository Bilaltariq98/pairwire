//! Typed connection wrapper — stub for P1.1/P1.2 implementation.
//!
//! Will wrap iroh's `Connection` to provide typed stream helpers and
//! datagram send/receive. Placeholder until iroh is wired in.

/// A pairwire connection to a remote peer.
///
/// Wraps the underlying QUIC connection and provides typed access to
/// control streams, input streams, and cursor datagrams.
///
/// Implemented in P1.1 (iroh endpoint setup) and P1.2 (stream helpers).
pub struct Connection {
    _private: (),
}

impl Connection {
    /// Placeholder — real implementation in P1.1.
    #[allow(dead_code)]
    pub(crate) fn placeholder() -> Self {
        Self { _private: () }
    }
}
