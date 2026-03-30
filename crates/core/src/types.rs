//! Fundamental identifier types used across all crates.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A peer's identity, derived from their iroh endpoint public key.
///
/// Wraps a 32-byte ed25519 public key. Used for addressing and authentication.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerId(pub [u8; 32]);

impl PeerId {
    /// Short display form: first 8 hex chars.
    #[must_use]
    pub fn short(&self) -> String {
        hex::encode(&self.0[..4])
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Unique identifier for a pairing session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub [u8; 16]);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_id_display() {
        let id = PeerId([0xab; 32]);
        assert!(id.to_string().starts_with("abab"));
        assert_eq!(id.short(), "abababab");
    }

    #[test]
    fn session_id_display() {
        let id = SessionId([0x01; 16]);
        assert_eq!(id.to_string(), "01010101010101010101010101010101");
    }
}
