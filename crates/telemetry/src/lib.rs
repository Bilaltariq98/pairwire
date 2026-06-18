//! Opt-in telemetry and crash reporting for pairwire.
//!
//! # Privacy contract
//!
//! - No event fires until [`set_consent`] is called with `true`.
//! - Collected: connection success/failure, session duration, app version, OS family, panics.
//! - Never collected: screen content, audio, keystrokes, invite tokens, NodeIds, IPs.
//! - The anonymous [`Uuid`] device ID is generated once and persisted to the OS config dir.
//!   It contains no user-identifying information.
//! - Crash reports (Sentry) honour the same opt-in as usage events (PostHog).

pub mod config;
pub mod consent;
pub mod events;
pub mod posthog;
pub mod sentry_init;

use tracing::{debug, warn};

pub use config::TelemetryConfig;
pub use consent::{consent_was_set, is_consented, set_consent};
pub use events::{EventSink, TelemetryEvent};
pub use sentry_init::init_sentry;

/// Initialise telemetry on startup.
///
/// Loads the persisted [`TelemetryConfig`] from the OS config dir. If consent
/// has been set previously it is restored into the process-global [`consent`]
/// gate so events resume immediately (no dialog needed on subsequent launches).
///
/// Returns the loaded/default config and the Sentry guard (must be held alive
/// for the process lifetime — drop it and Sentry stops capturing).
pub fn init() -> (TelemetryConfig, sentry::ClientInitGuard) {
    let cfg = match config::load() {
        Ok(c) => {
            debug!(
                consent = ?c.consent,
                device_id = %c.device_id,
                "telemetry config loaded"
            );
            c
        }
        Err(e) => {
            warn!(error = %e, "failed to load telemetry config, using defaults");
            TelemetryConfig::default()
        }
    };

    // Restore consent gate from persisted value (None = not yet decided = gate stays shut).
    if let Some(consented) = cfg.consent {
        consent::set_consent(consented);
    }

    let guard = sentry_init::init_sentry(cfg.device_id);
    (cfg, guard)
}

/// Fire a telemetry event through the provided sink if consent is granted.
///
/// The consent check is a single atomic load — effectively free on the hot
/// path when consent is not granted. This function is the single source of
/// truth for the consent gate, used by both production dispatch and tests.
pub fn record_with_sink(event: TelemetryEvent, sink: &impl EventSink) {
    if !is_consented() {
        return;
    }
    sink.send(event);
}

/// Fire a telemetry event if consent is granted.
///
/// Dispatches to the production PostHog EU sink. The check is a single atomic
/// load — effectively free on the hot path when consent is not granted.
pub fn record(event: TelemetryEvent) {
    record_with_sink(event, &posthog::PostHogSink);
}
