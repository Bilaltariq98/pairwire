//! Invite link encoding/decoding.
//!
//! Format: `pairwire://<hex-endpoint-id>?relay=<url>&token=<hex-session-token>`
//!
//! The endpoint ID is the iroh `EndpointId` (ed25519 public key), hex-encoded.
//! The relay URL is the host's home relay for fallback routing.
//! The token is a 16-byte session identifier for authentication.

use iroh::{EndpointAddr, EndpointId, RelayUrl};
use pairwire_core::{Error, Result, SessionId};
use std::fmt;
use std::str::FromStr;

/// An invite link that encodes everything a guest needs to connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteLink {
    /// The host's iroh endpoint address (ID + relay URL).
    pub endpoint_addr: EndpointAddr,
    /// Session token for authentication.
    pub session_id: SessionId,
}

impl InviteLink {
    /// Create an invite link from an endpoint address and session ID.
    #[must_use]
    pub fn new(endpoint_addr: EndpointAddr, session_id: SessionId) -> Self {
        Self {
            endpoint_addr,
            session_id,
        }
    }

    /// The host's endpoint ID (public key).
    #[must_use]
    pub fn endpoint_id(&self) -> &EndpointId {
        &self.endpoint_addr.id
    }
}

impl fmt::Display for InviteLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // endpoint ID is hex-encoded via its Display impl
        write!(f, "pairwire://{}", self.endpoint_addr.id)?;

        // Append relay URL if present
        let relay_urls: Vec<_> = self.endpoint_addr.relay_urls().collect();
        if let Some(relay) = relay_urls.first() {
            write!(f, "?relay={relay}")?;
            write!(f, "&token={}", hex::encode(self.session_id.0))?;
        } else {
            write!(f, "?token={}", hex::encode(self.session_id.0))?;
        }

        Ok(())
    }
}

impl FromStr for InviteLink {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s
            .strip_prefix("pairwire://")
            .ok_or_else(|| Error::Protocol("invite link must start with pairwire://".into()))?;

        let (id_str, query) = s
            .split_once('?')
            .ok_or_else(|| Error::Protocol("invite link missing query string".into()))?;

        // Parse endpoint ID from hex
        let endpoint_id: EndpointId = id_str
            .parse()
            .map_err(|e| Error::Protocol(format!("invalid endpoint id: {e}")))?;

        let mut relay_url_str = String::new();
        let mut token_hex = String::new();

        for param in query.split('&') {
            if let Some(v) = param.strip_prefix("relay=") {
                relay_url_str = v.to_string();
            } else if let Some(v) = param.strip_prefix("token=") {
                token_hex = v.to_string();
            }
        }

        if token_hex.len() != 32 {
            return Err(Error::Protocol(
                "invite link token must be 32 hex chars (16 bytes)".into(),
            ));
        }

        let token_bytes = hex::decode(&token_hex)
            .map_err(|e| Error::Protocol(format!("invalid token hex: {e}")))?;
        let mut session_bytes = [0u8; 16];
        session_bytes.copy_from_slice(&token_bytes);

        let mut endpoint_addr = EndpointAddr::new(endpoint_id);
        if !relay_url_str.is_empty() {
            let relay_url: RelayUrl = relay_url_str
                .parse()
                .map_err(|e| Error::Protocol(format!("invalid relay url: {e}")))?;
            endpoint_addr = endpoint_addr.with_relay_url(relay_url);
        }

        Ok(InviteLink {
            endpoint_addr,
            session_id: SessionId(session_bytes),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    fn make_invite() -> InviteLink {
        let key = SecretKey::generate(&mut rand::rng());
        let endpoint_id = key.public();
        let relay_url: RelayUrl = "https://relay.example.com".parse().unwrap();
        let endpoint_addr = EndpointAddr::new(endpoint_id).with_relay_url(relay_url);
        InviteLink::new(
            endpoint_addr,
            SessionId([0xde, 0xad, 0xbe, 0xef, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        )
    }

    #[test]
    fn invite_link_round_trip() {
        let link = make_invite();
        let s = link.to_string();
        assert!(s.starts_with("pairwire://"));
        assert!(s.contains("relay="));
        assert!(s.contains("token="));
        let parsed: InviteLink = s.parse().unwrap();
        assert_eq!(parsed.session_id, link.session_id);
        assert_eq!(parsed.endpoint_id(), link.endpoint_id());
    }

    #[test]
    fn invite_link_without_relay() {
        let key = SecretKey::generate(&mut rand::rng());
        let endpoint_addr = EndpointAddr::new(key.public());
        let link = InviteLink::new(endpoint_addr, SessionId([0x01; 16]));
        let s = link.to_string();
        let parsed: InviteLink = s.parse().unwrap();
        assert_eq!(parsed.session_id, link.session_id);
        assert_eq!(parsed.endpoint_id(), link.endpoint_id());
    }
}
