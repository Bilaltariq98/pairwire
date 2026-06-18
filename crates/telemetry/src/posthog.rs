//! PostHog EU event sink.
//!
//! # API key
//!
//! The API key is sourced (in priority order):
//! 1. `PAIRWIRE_POSTHOG_KEY` environment variable at runtime (override for CI/dev).
//! 2. `PAIRWIRE_POSTHOG_KEY` environment variable baked at compile time via
//!    `option_env!("PAIRWIRE_POSTHOG_KEY")`.
//!
//! If neither is set, PostHog is silently disabled (events are dropped with a
//! debug log). This prevents panics in builds that do not have the key wired.
//!
//! # EU endpoint
//!
//! All events go to `https://eu.i.posthog.com` (PostHog Cloud EU region).
//! This is GDPR-safe for EU testers. **Do not change this URL.**
//!
//! # distinct_id strategy
//!
//! A fresh UUID v4 is generated per-event. This means PostHog cannot correlate
//! events from the same device across sessions — by design, since persistent
//! device tracking requires a separate consent disclosure. If per-device session
//! counting is needed in a later version, pass `device_id` as `distinct_id` and
//! update the privacy notice accordingly.
//!
//! # posthog-rs 0.13.x API notes (verified 2026-06-18)
//!
//! The plan spec described a builder-style `ClientOptions::new(key).endpoint(EU_ENDPOINT)`
//! API returning `Result<Client, _>`. This does NOT match the actual 0.13.x API:
//!
//! ```text
//! pub async fn client<C: Into<ClientOptions>>(options: C) -> Client
//! ```
//!
//! Key differences from the spec template:
//! - `client()` is infallible — it returns `Client` directly, not `Result<Client, _>`.
//!   The `Err` branch from the spec (`warn!("failed to build PostHog client")`) is
//!   therefore not applicable and has been intentionally omitted (there is no error to
//!   handle at client construction time).
//! - `(&str, &str)` implements `From<(&str, &str)> for ClientOptions`, so the tuple
//!   `(api_key, host)` is the idiomatic way to pass a custom EU endpoint. No separate
//!   `ClientOptions::new().endpoint()` builder chain exists in this version.
//! - `Client::capture()` returns `Result<(), Error>`, so error handling at the capture
//!   step is preserved (see the `if let Err(e)` branch below).
//!
//! Source: `posthog-rs-0.13.1/src/client/async_client.rs` and `mod.rs`.
//! Re-verify with `cargo doc -p posthog-rs --open` if upgrading beyond 0.13.x.

use std::sync::OnceLock;

use tracing::{debug, warn};
use uuid::Uuid;

use crate::events::TelemetryEvent;

/// PostHog EU REST endpoint. Immutable — EU region only.
const EU_ENDPOINT: &str = "https://eu.i.posthog.com";

/// API key baked at compile time. May be `None` in dev builds.
const BAKED_API_KEY: Option<&str> = option_env!("PAIRWIRE_POSTHOG_KEY");

/// Process-level PostHog client, lazily initialised on first use.
///
/// Reusing a single client avoids spinning up a fresh `reqwest` connection pool
/// and a new TLS handshake on every event. Initialised at most once per process
/// via [`posthog_client()`].
static POSTHOG_CLIENT: OnceLock<posthog_rs::Client> = OnceLock::new();

/// Returns a reference to the shared PostHog client, initialising it on the
/// first call.
///
/// # Async initialisation with `OnceLock`
///
/// `OnceLock::get_or_init` is synchronous, so we cannot call the async
/// `posthog_rs::client()` directly inside it.  Instead we call
/// `tokio::runtime::Handle::current().block_on()` **only** when the cell is
/// empty (i.e., at most once per process lifetime), which is safe here because:
/// 1. We are always invoked from inside a `tokio::spawn` task, where blocking
///    briefly is acceptable (the spawned task runs on a thread-pool worker).
/// 2. `posthog_rs::client()` performs only lightweight reqwest builder work
///    (no I/O), so the blocking window is negligible.
///
/// An alternative would be `tokio::sync::OnceCell`, but that requires an
/// `async fn` call path all the way up to `send()` and would complicate the
/// fire-and-forget API surface without meaningful benefit.
fn posthog_client(key: &str) -> &'static posthog_rs::Client {
    // NOTE: client key is fixed at first successful call; runtime env-var changes
    // after init have no effect. Subsequent calls with a different `key` argument
    // silently reuse the client that was created on the first call.
    POSTHOG_CLIENT.get_or_init(|| {
        // `posthog_rs::client` is async but does no I/O during construction —
        // it only builds a reqwest::Client and fills in options.  Block on it
        // synchronously so we can fit into OnceLock::get_or_init.
        tokio::runtime::Handle::current().block_on(posthog_rs::client((key, EU_ENDPOINT)))
    })
}

/// Resolve the API key: runtime env override > compile-time baked value.
fn api_key() -> Option<String> {
    std::env::var("PAIRWIRE_POSTHOG_KEY")
        .ok()
        .or_else(|| BAKED_API_KEY.map(str::to_owned))
}

/// Fire a telemetry event to PostHog EU.
///
/// Non-blocking: spawns a `tokio::spawn` task for fire-and-forget dispatch.
///
/// **Pre-condition**: caller must check [`crate::consent::is_consented()`]
/// before calling `send`. This function does NOT re-check consent.
///
/// # Panics
///
/// Panics if called outside a tokio async runtime context. This function
/// internally calls [`tokio::spawn`], which requires an active tokio runtime.
/// Ensure a runtime (e.g. `#[tokio::main]`) is active before calling `send`.
pub fn send(event: TelemetryEvent) {
    let Some(key) = api_key() else {
        debug!(
            "PostHog key not set — event dropped: {}",
            event.event_name()
        );
        return;
    };

    let event_name = event.event_name().to_owned();

    // Build the property map from the event variant.
    let mut properties = std::collections::HashMap::<String, serde_json::Value>::new();
    match &event {
        TelemetryEvent::AppLaunched { version, os_family } => {
            properties.insert("version".into(), version.clone().into());
            properties.insert("os_family".into(), os_family.clone().into());
        }
        TelemetryEvent::ConnectionEstablished { relayed } => {
            properties.insert("relayed".into(), (*relayed).into());
        }
        TelemetryEvent::ConnectionFailed { reason } => {
            properties.insert("reason".into(), reason.clone().into());
        }
        TelemetryEvent::SessionEnded { duration_secs } => {
            properties.insert("duration_secs".into(), (*duration_secs).into());
        }
    }

    // tokio::spawn for fire-and-forget dispatch. The task is detached; any
    // failures are logged at debug/warn level and do not propagate to the caller.
    tokio::spawn(async move {
        // Obtain (or lazily initialise) the shared, process-level PostHog client.
        // posthog_client() reuses the existing reqwest connection pool — no new
        // TLS handshake per event. See posthog_client() doc for the rationale on
        // why OnceLock + block_on is used rather than tokio::sync::OnceCell.
        let client = posthog_client(key.as_str());

        // Fresh UUID per event — see module doc on distinct_id strategy.
        let distinct_id = Uuid::new_v4().to_string();

        let mut ph_event = posthog_rs::Event::new(event_name.as_str(), distinct_id.as_str());
        for (k, v) in properties {
            if let Err(e) = ph_event.insert_prop(k, v) {
                warn!(error = %e, "failed to insert PostHog property");
            }
        }

        if let Err(e) = client.capture(ph_event).await {
            debug!(error = %e, "PostHog capture failed (non-fatal)");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::TELEMETRY_TEST_LOCK;

    /// When no API key is set (env var absent, no baked key), `send` must
    /// return immediately without panicking or spawning a tokio task.
    ///
    /// BAKED_API_KEY is `None` in dev builds (env var not set at compile time),
    /// so we only need to ensure the runtime env var is also absent.
    ///
    /// If BAKED_API_KEY IS set at compile time (e.g. release CI), `send` would
    /// call `tokio::spawn`, which panics outside a tokio runtime. This test
    /// skips the `send` call in that case to avoid the panic — the no-key drop
    /// path is only reachable when BAKED_API_KEY is None.
    #[test]
    fn send_drops_event_silently_when_no_key() {
        let _guard = TELEMETRY_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // SAFETY: tests serialise on TELEMETRY_TEST_LOCK so only one thread
        // touches this env var at a time — no concurrent env mutation risk.
        unsafe { std::env::remove_var("PAIRWIRE_POSTHOG_KEY") };

        // When a baked key is present, api_key() returns Some(_) even after the
        // env var is removed. Calling send() would invoke tokio::spawn from a
        // plain #[test] with no tokio runtime, causing a panic. Skip the call
        // in that case — this test only validates the no-key drop path.
        if BAKED_API_KEY.is_some() {
            return;
        }

        // No baked key and no runtime env var: send() must drop the event
        // immediately (debug-logged) without panicking or spawning a task.
        send(TelemetryEvent::AppLaunched {
            version: "0.0.0-test".into(),
            os_family: "test".into(),
        });
        // If we reach here, no panic occurred — the drop path works correctly.
    }

    /// `api_key()` returns the runtime env var when set, taking priority over
    /// any baked compile-time value.
    #[test]
    fn api_key_runtime_env_overrides_baked() {
        let _guard = TELEMETRY_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // SAFETY: tests serialise on TELEMETRY_TEST_LOCK so only one thread
        // touches this env var at a time — no concurrent env mutation risk.
        unsafe { std::env::set_var("PAIRWIRE_POSTHOG_KEY", "test-runtime-key") };
        let key = api_key();
        unsafe { std::env::remove_var("PAIRWIRE_POSTHOG_KEY") };

        assert_eq!(
            key,
            Some("test-runtime-key".to_owned()),
            "runtime env var must take priority"
        );
    }

    /// `api_key()` returns `None` when both the runtime env var and the baked
    /// compile-time key are absent (standard dev build without key).
    #[test]
    fn api_key_returns_none_when_unset() {
        let _guard = TELEMETRY_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        // SAFETY: tests serialise on TELEMETRY_TEST_LOCK so only one thread
        // touches this env var at a time — no concurrent env mutation risk.
        unsafe { std::env::remove_var("PAIRWIRE_POSTHOG_KEY") };

        // BAKED_API_KEY is None in dev builds (no env var at compile time).
        // If a CI build sets PAIRWIRE_POSTHOG_KEY at compile time, this test
        // will correctly return Some(_) and the assertion guards only the None
        // branch. We skip in that case with a soft check.
        if BAKED_API_KEY.is_none() {
            assert!(
                api_key().is_none(),
                "api_key() must return None when no key is configured at all"
            );
        }
        // else: baked key present → api_key() returns Some(_); that's correct.
        // No assertion needed — the function's behaviour is still valid.
    }
}
