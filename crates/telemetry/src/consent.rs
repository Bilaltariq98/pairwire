//! Process-global consent gate for telemetry.
//!
//! All event emission is gated behind `is_consented()`, which does a single
//! atomic load. The default state (not yet set) evaluates as `false` —
//! **nothing is sent before the user explicitly opts in**.
//!
//! Call [`set_consent`] with `true` to enable, `false` to disable. Persisting
//! the choice to disk is the caller's responsibility (see [`super::config`]).

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the user has been asked and consented.
static CONSENT: AtomicBool = AtomicBool::new(false);
/// Whether `set_consent` has been called at all.
///
/// Allows distinguishing "not yet decided" (`false`) from "explicitly opted out"
/// (`true` + `CONSENT = false`).
static CONSENT_SET: AtomicBool = AtomicBool::new(false);

/// Returns `true` only if the user has explicitly opted in.
///
/// Thread-safe; uses `Ordering::Relaxed` — telemetry correctness is not
/// safety-critical and a transient wrong read costs at most one extra/missing
/// event.
#[inline]
pub fn is_consented() -> bool {
    CONSENT.load(Ordering::Relaxed)
}

/// Returns `true` if consent has been explicitly set (either direction).
///
/// Use this to decide whether to show the first-run dialog.
#[inline]
pub fn consent_was_set() -> bool {
    CONSENT_SET.load(Ordering::Relaxed)
}

/// Set the consent flag. Persisting to disk is the caller's responsibility.
///
/// May be called from any thread.
pub fn set_consent(consented: bool) {
    CONSENT.store(consented, Ordering::Relaxed);
    CONSENT_SET.store(true, Ordering::Relaxed);
    tracing::info!(consented, "telemetry consent updated");
}

/// Reset consent to the unset state. Only used in tests.
#[cfg(any(test, feature = "test-support"))]
pub fn reset_for_test() {
    CONSENT.store(false, Ordering::Relaxed);
    CONSENT_SET.store(false, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_reset<F: FnOnce()>(f: F) {
        reset_for_test();
        f();
        reset_for_test();
    }

    #[test]
    fn default_is_not_consented() {
        with_reset(|| {
            assert!(
                !is_consented(),
                "must default to false — no PII before opt-in"
            );
        });
    }

    #[test]
    fn set_true_enables_consent() {
        with_reset(|| {
            set_consent(true);
            assert!(is_consented());
            assert!(consent_was_set());
        });
    }

    #[test]
    fn set_false_disables_consent() {
        with_reset(|| {
            set_consent(false);
            assert!(!is_consented());
            assert!(
                consent_was_set(),
                "consent_was_set must be true even for opt-out"
            );
        });
    }

    #[test]
    fn toggle_consent() {
        with_reset(|| {
            set_consent(true);
            assert!(is_consented());
            set_consent(false);
            assert!(!is_consented());
            set_consent(true);
            assert!(is_consented());
        });
    }
}
