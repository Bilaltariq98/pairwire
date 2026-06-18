//! Allowlist of telemetry events and the `EventSink` abstraction.
//!
//! # Denylist (enforced by type system — these fields do not exist)
//!
//! The enum variants **cannot** carry:
//! - Screen content, audio samples, or keystrokes
//! - Invite tokens or session URLs
//! - iroh NodeIds / PublicKeys
//! - IP addresses
//!
//! Any attempt to add those fields will surface in code review and must be
//! rejected.

use serde::{Deserialize, Serialize};

/// Telemetry events that Pairwire is allowed to emit.
///
/// Keep the list minimal. Every addition requires explicit justification in
/// the PR description and must not carry any of the denylisted fields above.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TelemetryEvent {
    /// The app process started successfully.
    AppLaunched {
        /// Semantic version string from `env!("CARGO_PKG_VERSION")`.
        version: String,
        /// OS family: "macos", "linux", or "windows".
        os_family: String,
    },
    /// A P2P QUIC connection was established with a peer.
    ///
    /// Carries no NodeId, no invite token, no IP.
    ConnectionEstablished {
        /// Whether the connection used the relay (true) or was direct (false).
        /// Derived from iroh path info; never carries the relay URL or peer ID.
        relayed: bool,
    },
    /// A connection attempt failed.
    ///
    /// `reason` is a short programmer-readable tag, not a user-facing string.
    /// Never include invite tokens, URLs, or addresses in `reason`.
    ConnectionFailed {
        /// Short tag: "timeout", "protocol_error", "invite_parse_error", etc.
        reason: String,
    },
    /// A session ended (host or guest side).
    SessionEnded {
        /// Duration in whole seconds.
        duration_secs: u64,
    },
}

impl TelemetryEvent {
    /// Returns the PostHog event name (used as the `event` field in the API).
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::AppLaunched { .. } => "app_launched",
            Self::ConnectionEstablished { .. } => "connection_established",
            Self::ConnectionFailed { .. } => "connection_failed",
            Self::SessionEnded { .. } => "session_ended",
        }
    }
}

/// Abstraction over the telemetry backend, enabling tests to use a fake sink
/// without real network I/O.
pub trait EventSink: Send + Sync + 'static {
    /// Send an event. Implementations must be non-blocking (fire-and-forget is fine).
    fn send(&self, event: TelemetryEvent);
}

/// A fake sink that records events in memory for test assertions.
#[cfg(any(test, feature = "test-support"))]
pub mod fake {
    use super::{EventSink, TelemetryEvent};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, Default)]
    pub struct FakeSink {
        pub events: Arc<Mutex<Vec<TelemetryEvent>>>,
    }

    impl FakeSink {
        pub fn new() -> Self {
            Self::default()
        }

        /// Drain all captured events and return them.
        pub fn drain(&self) -> Vec<TelemetryEvent> {
            self.events
                .lock()
                .expect("FakeSink mutex poisoned")
                .drain(..)
                .collect()
        }

        /// Number of captured events.
        pub fn len(&self) -> usize {
            self.events.lock().expect("FakeSink mutex poisoned").len()
        }

        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }
    }

    impl EventSink for FakeSink {
        fn send(&self, event: TelemetryEvent) {
            self.events
                .lock()
                .expect("FakeSink mutex poisoned")
                .push(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeSink;
    use super::*;
    use crate::consent;

    /// Consent gate suppresses all events when consent is None/false.
    ///
    /// This test verifies the core privacy guarantee: nothing is sent before
    /// the user opts in.
    #[test]
    fn consent_gate_suppresses_events_when_not_consented() {
        consent::reset_for_test();
        // consent is NOT set → gate is shut

        let sink = FakeSink::new();

        // Simulate what `crate::record()` does, but using the FakeSink directly
        // so we don't need the global PostHog client.
        let check_and_send = |event: TelemetryEvent| {
            if consent::is_consented() {
                sink.send(event);
            }
        };

        check_and_send(TelemetryEvent::AppLaunched {
            version: "0.1.0".into(),
            os_family: "macos".into(),
        });
        check_and_send(TelemetryEvent::ConnectionEstablished { relayed: false });
        check_and_send(TelemetryEvent::ConnectionFailed {
            reason: "timeout".into(),
        });
        check_and_send(TelemetryEvent::SessionEnded { duration_secs: 300 });

        assert!(
            sink.is_empty(),
            "no events must be sent when consent is not granted; got {} events",
            sink.len()
        );

        consent::reset_for_test();
    }

    /// Consent gate allows events when consented.
    #[test]
    fn consent_gate_allows_events_when_consented() {
        consent::reset_for_test();
        consent::set_consent(true);

        let sink = FakeSink::new();

        let check_and_send = |event: TelemetryEvent| {
            if consent::is_consented() {
                sink.send(event);
            }
        };

        check_and_send(TelemetryEvent::AppLaunched {
            version: "0.1.0".into(),
            os_family: "macos".into(),
        });
        check_and_send(TelemetryEvent::ConnectionEstablished { relayed: true });

        assert_eq!(
            sink.len(),
            2,
            "both events must reach the sink when consented"
        );

        consent::reset_for_test();
    }

    /// Event payloads must not contain any denylisted fields.
    ///
    /// Serialise each variant and assert the JSON output does not contain
    /// strings that look like NodeIds, invite tokens, IPs, or screen data.
    #[test]
    fn event_payloads_contain_no_denylisted_fields() {
        let events = vec![
            TelemetryEvent::AppLaunched {
                version: "0.1.0".into(),
                os_family: "macos".into(),
            },
            TelemetryEvent::ConnectionEstablished { relayed: false },
            TelemetryEvent::ConnectionFailed {
                reason: "timeout".into(),
            },
            TelemetryEvent::SessionEnded { duration_secs: 600 },
        ];

        let denylisted_keys = [
            "node_id",
            "nodeid",
            "invite",
            "token",
            "ip_address",
            "endpoint_id",
            "public_key",
            "screen",
            "audio",
            "keystroke",
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap_or_else(|_| format!("{:?}", event));
            let json_lower = json.to_lowercase();
            for key in &denylisted_keys {
                assert!(
                    !json_lower.contains(key),
                    "event {:?} serialised payload contains denylisted key '{}': {}",
                    event.event_name(),
                    key,
                    json
                );
            }
        }
    }

    /// `event_name()` returns a non-empty string for every variant.
    #[test]
    fn event_name_is_non_empty_for_all_variants() {
        let events = vec![
            TelemetryEvent::AppLaunched {
                version: "0.1.0".into(),
                os_family: "macos".into(),
            },
            TelemetryEvent::ConnectionEstablished { relayed: false },
            TelemetryEvent::ConnectionFailed {
                reason: "timeout".into(),
            },
            TelemetryEvent::SessionEnded { duration_secs: 0 },
        ];
        for event in &events {
            assert!(!event.event_name().is_empty());
        }
    }
}
