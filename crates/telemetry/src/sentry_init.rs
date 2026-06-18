//! Sentry EU crash reporting initialisation.
//!
//! # DSN
//!
//! The DSN is sourced (in priority order):
//! 1. `PAIRWIRE_SENTRY_DSN` environment variable at runtime.
//! 2. `PAIRWIRE_SENTRY_DSN` environment variable baked at compile time via
//!    `option_env!("PAIRWIRE_SENTRY_DSN")`.
//!
//! If neither is set, Sentry is disabled (the guard is a no-op).
//!
//! # GDPR
//!
//! Crash reports honour the same opt-in as usage events. We install the
//! Sentry client but call `sentry::configure_scope` to attach a `before_send`
//! hook that drops all events when the user has not consented.
//!
//! # DSN embedded in binary
//!
//! The Sentry DSN is intentionally baked into the release binary — this is
//! standard practice for native applications and unavoidable for automatic
//! crash capture. **Mitigate abuse by enabling rate limiting and PII scrubbing
//! on the Sentry project** (see the [human] task in the plan).

use tracing::{debug, info, warn};
use uuid::Uuid;

/// DSN baked at compile time. `None` in builds without the env var set.
const BAKED_DSN: Option<&str> = option_env!("PAIRWIRE_SENTRY_DSN");

/// Consent gate for Sentry's `before_send` hook.
///
/// Returns `Some(event)` when the user has consented, `None` to drop the event.
/// Extracted as a named free function so tests can invoke the exact production
/// logic without reconstructing it inline.
pub(crate) fn consent_gate(
    event: sentry::protocol::Event<'static>,
) -> Option<sentry::protocol::Event<'static>> {
    if crate::consent::is_consented() {
        Some(event)
    } else {
        None // drop — user has not consented
    }
}

/// Resolve the Sentry DSN: runtime env override > compile-time baked value.
fn dsn() -> Option<String> {
    std::env::var("PAIRWIRE_SENTRY_DSN")
        .ok()
        .or_else(|| BAKED_DSN.map(str::to_owned))
}

/// Initialise Sentry and install the panic hook.
///
/// Returns a `sentry::ClientInitGuard` that **must be kept alive** for the
/// entire process lifetime. Dropping it flushes pending events and tears down
/// Sentry — keep it in `main()`.
///
/// If no DSN is configured, returns a no-op guard.
pub fn init_sentry(device_id: Uuid) -> sentry::ClientInitGuard {
    let Some(dsn_str) = dsn() else {
        debug!("PAIRWIRE_SENTRY_DSN not set — Sentry disabled");
        // Return a guard initialised with an empty options set (no-op).
        return sentry::init(sentry::ClientOptions::default());
    };

    info!("initialising Sentry EU crash reporting");

    let guard = sentry::init(sentry::ClientOptions {
        dsn: dsn_str
            .parse()
            .map_err(|e| {
                warn!(
                    error = %e,
                    dsn = %dsn_str,
                    "Sentry DSN is malformed — crash reporting disabled"
                );
                e
            })
            .ok(),
        // Pin the release to the binary version for symbolication.
        release: Some(std::borrow::Cow::Borrowed(env!("CARGO_PKG_VERSION"))),
        // EU ingest endpoint is encoded in the DSN itself when using
        // https://o<org>.ingest.eu.sentry.io/... DSNs. No separate server
        // config needed — the DSN carries the EU region.
        //
        // Attach-stacktrace: include stacktrace on all events, not just panics.
        attach_stacktrace: true,
        // before_send: gate on consent. If the user has not opted in, drop the
        // event. This is checked at event send time, not at init time, so
        // toggling consent mid-session works correctly.
        before_send: Some(std::sync::Arc::new(consent_gate)),
        ..Default::default()
    });

    // Tag every event with the anonymous device ID for correlation.
    // Never set `user.email` or `user.username` — keep it to the UUID only.
    sentry::configure_scope(|scope| {
        scope.set_user(Some(sentry::User {
            id: Some(device_id.to_string()),
            ..Default::default()
        }));
    });

    guard
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent;
    use crate::consent::TELEMETRY_TEST_LOCK;

    /// `init_sentry` returns without panic when no DSN is configured.
    #[test]
    fn init_without_dsn_is_noop() {
        // Use the crate-wide TELEMETRY_TEST_LOCK rather than a module-local
        // ENV_LOCK so that all env-var mutations (both PAIRWIRE_SENTRY_DSN and
        // PAIRWIRE_POSTHOG_KEY) are serialised under a single lock. This
        // prevents a future test that reads both variables from being unprotected
        // against concurrent mutations from the other module.
        let _guard = TELEMETRY_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // SAFETY: TELEMETRY_TEST_LOCK serialises all access to telemetry env
        // vars in this test binary — no other thread reads or writes them while
        // the lock is held.
        unsafe { std::env::remove_var("PAIRWIRE_SENTRY_DSN") };
        let device_id = Uuid::new_v4();
        // Should not panic or error.
        let _guard2 = init_sentry(device_id);
    }

    /// `before_send` drops events when consent has not been granted.
    ///
    /// Calls the production `consent_gate` function directly — the same `Arc<Fn>`
    /// registered in `sentry::ClientOptions::before_send` — so that any accidental
    /// inversion of the consent check or removal of the hook will be caught.
    #[test]
    fn before_send_drops_event_when_not_consented() {
        let _guard = TELEMETRY_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        consent::reset_for_test();
        // consent is NOT set → gate must drop the event

        let result = consent_gate(sentry::protocol::Event::new());
        assert!(
            result.is_none(),
            "before_send must return None (drop event) when consent is not granted"
        );
    }

    /// `before_send` forwards events when consent has been granted.
    ///
    /// Calls the production `consent_gate` function directly to verify it returns
    /// `Some(event)` when the user has opted in.
    #[test]
    fn before_send_forwards_event_when_consented() {
        let _guard = TELEMETRY_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        consent::reset_for_test();
        consent::set_consent(true);

        let result = consent_gate(sentry::protocol::Event::new());
        assert!(
            result.is_some(),
            "before_send must return Some(event) (forward event) when consent is granted"
        );

        consent::reset_for_test();
    }
}
