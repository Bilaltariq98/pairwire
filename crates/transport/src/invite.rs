//! Invite link encoding/decoding.
//!
//! Format: `pairwire://<base32-endpoint-id>?relay=<url>&token=<hex-session-token>`

use pairwire_core::{Error, Result, SessionId};
use std::fmt;
use std::str::FromStr;

/// An invite link that encodes everything a guest needs to connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteLink {
    /// The host's iroh endpoint ID (32-byte public key), base32-encoded.
    pub endpoint_id_b32: String,
    /// Relay server URL for fallback routing.
    pub relay_url: String,
    /// Session token for authentication.
    pub session_id: SessionId,
}

impl fmt::Display for InviteLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pairwire://{}?relay={}&token={}",
            self.endpoint_id_b32,
            self.relay_url,
            hex::encode(self.session_id.0),
        )
    }
}

impl FromStr for InviteLink {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        // Minimal parser — full URL parsing added in P1.1
        let s = s
            .strip_prefix("pairwire://")
            .ok_or_else(|| Error::Protocol("invite link must start with pairwire://".into()))?;

        let (endpoint_part, query) = s
            .split_once('?')
            .ok_or_else(|| Error::Protocol("invite link missing query string".into()))?;

        let mut relay_url = String::new();
        let mut token_hex = String::new();

        for param in query.split('&') {
            if let Some(v) = param.strip_prefix("relay=") {
                relay_url = v.to_string();
            } else if let Some(v) = param.strip_prefix("token=") {
                token_hex = v.to_string();
            }
        }

        if relay_url.is_empty() {
            return Err(Error::Protocol("invite link missing relay param".into()));
        }
        if token_hex.len() != 32 {
            return Err(Error::Protocol(
                "invite link token must be 32 hex chars".into(),
            ));
        }

        let token_bytes = hex::decode(&token_hex)
            .map_err(|e| Error::Protocol(format!("invalid token hex: {e}")))?;
        let mut session_bytes = [0u8; 16];
        session_bytes.copy_from_slice(&token_bytes);

        Ok(InviteLink {
            endpoint_id_b32: endpoint_part.to_string(),
            relay_url,
            session_id: SessionId(session_bytes),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_link_round_trip() {
        let link = InviteLink {
            endpoint_id_b32: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            relay_url: "https://relay.example.com".to_string(),
            session_id: SessionId([0xde, 0xad, 0xbe, 0xef, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        };
        let s = link.to_string();
        assert!(s.starts_with("pairwire://"));
        let parsed: InviteLink = s.parse().unwrap();
        assert_eq!(parsed, link);
    }
}
