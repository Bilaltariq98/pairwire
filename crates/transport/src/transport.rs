//! iroh Endpoint wrapper — singleton transport for a pairwire process.
//!
//! Create exactly one `Transport` per process. It owns the iroh `Endpoint`
//! and provides `host()` / `join()` entry points.

use std::time::Duration;

use iroh::{
    Endpoint, RelayMode, RelayUrl,
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

/// Pure, testable relay-mode selector.
///
/// Precedence (highest first):
/// 1. `env_url`  — explicit `PAIRWIRE_RELAY_URL` value → `Custom(vec![url])`
/// 2. `env_mode` — `PAIRWIRE_RELAY` keyword:
///    `"public"` → `Default` (n0 public), `"disabled"` → `Disabled`,
///    `"ours"` → `Custom(vec![our_relay])`
/// 3. Build-profile default (no env):
///    `is_debug=true` → `Default` (n0 public, for internal dogfooding)
///    `is_debug=false` → `Custom(vec![our_relay])` — **our relay only, no n0 in map**
///
/// URL validation is strict: only `https` scheme is accepted for explicit URLs.
///
/// # Errors
///
/// Returns an error if `env_url` is `Some` but cannot be parsed as a `RelayUrl` or uses a
/// non-https scheme, or if `env_mode` is `Some` but not a recognised keyword.
fn select_relay_mode(
    env_url: Option<&str>,
    env_mode: Option<&str>,
    is_debug: bool,
    our_relay: &str,
) -> Result<RelayMode> {
    // 1. Explicit URL override.
    if let Some(url_str) = env_url {
        let parsed: RelayUrl = url_str.parse().map_err(|e| {
            Error::Transport(format!(
                "PAIRWIRE_RELAY_URL '{url_str}' is not a valid relay URL: {e}"
            ))
        })?;
        // Reject non-https schemes — relay servers must use TLS.
        if parsed.scheme() != "https" {
            return Err(Error::Transport(format!(
                "PAIRWIRE_RELAY_URL '{url_str}' uses scheme '{}'; only 'https' is accepted",
                parsed.scheme()
            )));
        }
        let map = iroh::RelayMap::from_iter([parsed]);
        return Ok(RelayMode::Custom(map));
    }

    // 2. Keyword env override.
    if let Some(kw) = env_mode {
        return match kw {
            "public" => Ok(RelayMode::Default),
            "disabled" => Ok(RelayMode::Disabled),
            "ours" => {
                let url: RelayUrl = our_relay.parse().map_err(|e| {
                    Error::Transport(format!(
                        "compiled-in relay URL '{our_relay}' is invalid: {e}"
                    ))
                })?;
                Ok(RelayMode::Custom(iroh::RelayMap::from_iter([url])))
            }
            other => Err(Error::Transport(format!(
                "PAIRWIRE_RELAY='{other}' is not a recognised value; \
                 use 'public', 'disabled', or 'ours'"
            ))),
        };
    }

    // 3. Build-profile default.
    if is_debug {
        // Debug/internal: use n0's public relays so cargo-run and debug DMGs
        // work without standing up our relay server.
        tracing::debug!("relay_mode=Default (debug build, no env override)");
        Ok(RelayMode::Default)
    } else {
        // Release: our relay only. End-user traffic must not touch n0's public network.
        let url: RelayUrl = our_relay.parse().map_err(|e| {
            Error::Transport(format!(
                "compiled-in relay URL '{our_relay}' is invalid: {e}"
            ))
        })?;
        tracing::info!(
            relay_url = %url,
            total_relays = 1u32,
            "relay_mode=Custom(ours-only, release build, no n0 in map)"
        );
        Ok(RelayMode::Custom(iroh::RelayMap::from_iter([url])))
    }
}

/// Reads `PAIRWIRE_RELAY_URL` and `PAIRWIRE_RELAY` from the environment,
/// passes `cfg!(debug_assertions)` as the build-profile flag, and delegates
/// to [`select_relay_mode`].
///
/// This is the only function that touches `std::env` — keeping env reads out
/// of the pure inner function makes it trivially unit-testable.
///
/// # Errors
///
/// Propagates errors from [`select_relay_mode`] (malformed URL, unknown keyword).
pub fn resolve_relay_mode() -> Result<RelayMode> {
    let env_url = std::env::var("PAIRWIRE_RELAY_URL").ok();
    let env_mode = std::env::var("PAIRWIRE_RELAY").ok();
    select_relay_mode(
        env_url.as_deref(),
        env_mode.as_deref(),
        cfg!(debug_assertions),
        "https://relay.pairwire.dev",
    )
}

/// The pairwire transport layer. Owns the iroh `Endpoint` singleton.
pub struct Transport {
    endpoint: Endpoint,
}

impl Transport {
    /// Bind a new iroh endpoint using the relay mode from [`resolve_relay_mode`].
    ///
    /// Relay mode is determined by environment variables and the build profile —
    /// see [`resolve_relay_mode`] for precedence. Call sites pass no relay argument;
    /// all configuration is via env or build flags.
    ///
    /// Configures a 2-second keepalive interval and 5-second max idle timeout
    /// so that dead peers are detected within ~5 seconds rather than the QUIC
    /// default of ~30 seconds.
    ///
    /// # Errors
    ///
    /// Returns an error if relay mode resolution fails (malformed URL, unknown env
    /// value) or if the endpoint cannot be bound (e.g., port conflict).
    #[instrument(name = "transport_bind")]
    pub async fn bind() -> Result<Self> {
        let idle_timeout = IdleTimeout::try_from(TRANSPORT_MAX_IDLE_TIMEOUT)
            .map_err(|e| Error::Transport(format!("invalid idle timeout: {e}")))?;

        let transport_config = QuicTransportConfig::builder()
            .keep_alive_interval(TRANSPORT_KEEP_ALIVE_INTERVAL)
            .max_idle_timeout(Some(idle_timeout))
            .build();

        let relay_mode = resolve_relay_mode()?;

        let endpoint = Endpoint::builder(presets::N0)
            .relay_mode(relay_mode)
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

#[cfg(test)]
mod relay_mode_tests {
    use super::select_relay_mode;

    const OUR_RELAY: &str = "https://relay.pairwire.dev";

    // 1. Explicit URL override (any build, debug or release)
    #[test]
    fn explicit_url_override_returns_custom() {
        let mode = select_relay_mode(
            Some("https://custom.relay.example.com"),
            None,
            true, // is_debug irrelevant when URL set
            OUR_RELAY,
        )
        .expect("valid https URL must succeed");
        // Must be Custom with exactly the supplied URL (not OUR_RELAY, not n0).
        match mode {
            iroh::RelayMode::Custom(map) => {
                assert_eq!(
                    map.len(),
                    1,
                    "explicit override must produce exactly 1 relay"
                );
                let url: iroh::RelayUrl = "https://custom.relay.example.com".parse().unwrap();
                assert!(
                    map.contains(&url),
                    "custom map must contain the supplied URL"
                );
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    // 2. PAIRWIRE_RELAY=public → Default (n0 public)
    #[test]
    fn env_mode_public_returns_default() {
        let mode =
            select_relay_mode(None, Some("public"), false, OUR_RELAY).expect("public must succeed");
        assert!(
            matches!(mode, iroh::RelayMode::Default),
            "PAIRWIRE_RELAY=public must yield RelayMode::Default, got {mode:?}"
        );
    }

    // 3. PAIRWIRE_RELAY=disabled → Disabled
    #[test]
    fn env_mode_disabled_returns_disabled() {
        let mode = select_relay_mode(None, Some("disabled"), false, OUR_RELAY)
            .expect("disabled must succeed");
        assert!(
            matches!(mode, iroh::RelayMode::Disabled),
            "PAIRWIRE_RELAY=disabled must yield RelayMode::Disabled, got {mode:?}"
        );
    }

    // 4. PAIRWIRE_RELAY=ours → Custom(ours-only, no n0)
    #[test]
    fn env_mode_ours_returns_custom_ours_only() {
        let mode =
            select_relay_mode(None, Some("ours"), false, OUR_RELAY).expect("ours must succeed");
        match mode {
            iroh::RelayMode::Custom(map) => {
                assert_eq!(map.len(), 1, "ours must produce exactly 1 relay (no n0)");
                let url: iroh::RelayUrl = OUR_RELAY.parse().unwrap();
                assert!(map.contains(&url));
            }
            other => panic!("expected Custom(ours), got {other:?}"),
        }
    }

    // 5. Debug default (is_debug=true, no env) → Default (n0 public, for dogfooding)
    #[test]
    fn debug_default_returns_relay_mode_default() {
        let mode =
            select_relay_mode(None, None, true, OUR_RELAY).expect("debug default must succeed");
        assert!(
            matches!(mode, iroh::RelayMode::Default),
            "debug build with no env must yield RelayMode::Default (n0 public), got {mode:?}"
        );
    }

    // 6. Release default (is_debug=false, no env) → Custom(ours-only, exactly 1 relay, no n0)
    #[test]
    fn release_default_returns_custom_ours_only_one_relay() {
        let mode =
            select_relay_mode(None, None, false, OUR_RELAY).expect("release default must succeed");
        match mode {
            iroh::RelayMode::Custom(map) => {
                assert_eq!(
                    map.len(),
                    1,
                    "release default must have exactly 1 relay (our relay only, no n0 in map)"
                );
                let url: iroh::RelayUrl = OUR_RELAY.parse().unwrap();
                assert!(
                    map.contains(&url),
                    "release map must contain relay.pairwire.dev"
                );
            }
            other => panic!("expected Custom(ours-only), got {other:?}"),
        }
    }

    // 7. Malformed URL → Err
    #[test]
    fn malformed_url_returns_err() {
        let result = select_relay_mode(Some("not_a_url"), None, false, OUR_RELAY);
        assert!(
            result.is_err(),
            "malformed URL must return Err, got Ok({:?})",
            result
        );
    }

    // 8. Non-https scheme in explicit URL → Err (select_relay_mode rejects non-https)
    #[test]
    fn non_https_explicit_url_returns_err() {
        let result = select_relay_mode(Some("ftp://relay.example.com"), None, false, OUR_RELAY);
        assert!(result.is_err(), "non-https URL must be rejected");
    }

    // 9. Unknown PAIRWIRE_RELAY value → Err
    #[test]
    fn unknown_env_mode_returns_err() {
        let result = select_relay_mode(None, Some("blah"), false, OUR_RELAY);
        assert!(
            result.is_err(),
            "unknown PAIRWIRE_RELAY value must return Err"
        );
    }
}
