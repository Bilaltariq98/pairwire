//! Unified error type for pairwire library crates.
//!
//! Use `thiserror` to derive `std::error::Error`. Binary/app crate uses `anyhow` instead.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("transport error: {0}")]
    Transport(String),

    #[error("capture error: {0}")]
    Capture(String),

    #[error("encode error: {0}")]
    Encode(String),

    #[error("decode error: {0}")]
    Decode(String),

    #[error("audio error: {0}")]
    Audio(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("serialization error: {0}")]
    Postcard(#[from] postcard::Error),

    #[error("connection closed")]
    ConnectionClosed,

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("version mismatch: local={local}, remote={remote}")]
    VersionMismatch { local: u16, remote: u16 },

    #[error("peer authentication failed: expected {expected}, got {actual}")]
    PeerAuthFailed { expected: String, actual: String },

    #[error("config validation: {0}")]
    InvalidConfig(String),
}

pub type Result<T> = std::result::Result<T, Error>;
