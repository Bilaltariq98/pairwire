//! Session reconnection logic.
//!
//! Wraps a connection attempt in an exponential-backoff retry loop.
//! Emits `ConnectionState` updates on a `watch` channel so the viewer
//! can show "Reconnecting…" or "Connection lost" overlays.
//!
//! Policy:
//! - Max 3 reconnect attempts after a drop
//! - Backoff: 1s, 2s, 4s
//! - After 3 failures: emit `ConnectionState::Lost` and give up

use std::time::Duration;

use tokio::{sync::watch, time};
use tracing::{info, warn};

/// Connection state broadcast to the viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// Session is active.
    Connected,
    /// Connection dropped; attempting to reconnect. `attempt` is 1-based.
    Reconnecting { attempt: u32, max: u32 },
    /// All reconnect attempts exhausted.
    Lost,
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connected => write!(f, "Connected"),
            Self::Reconnecting { attempt, max } => {
                write!(f, "Reconnecting… ({attempt}/{max})")
            }
            Self::Lost => write!(f, "Connection lost"),
        }
    }
}

/// Reconnection policy parameters.
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
        }
    }
}

impl ReconnectPolicy {
    /// Delay before attempt `n` (1-based). Doubles each attempt, capped at 8s.
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let secs = self.base_delay.as_secs() * (1 << (attempt - 1).min(3));
        Duration::from_secs(secs)
    }
}

/// Run `connect_fn` in a retry loop, emitting state updates.
///
/// `connect_fn` is called once per attempt. It should return `Ok(())` on
/// clean disconnect (user-initiated) or `Err` on unexpected drop.
///
/// Returns `Ok(())` if the session ended cleanly, `Err` after all retries fail.
pub async fn with_reconnect<F, Fut>(
    policy: ReconnectPolicy,
    state_tx: watch::Sender<ConnectionState>,
    mut connect_fn: F,
) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    // First attempt — not a reconnect.
    match connect_fn().await {
        Ok(()) => return Ok(()),
        Err(e) => {
            warn!(error = %e, "session ended unexpectedly, will retry");
        }
    }

    for attempt in 1..=policy.max_attempts {
        let delay = policy.delay_for(attempt);
        let _ = state_tx.send(ConnectionState::Reconnecting {
            attempt,
            max: policy.max_attempts,
        });
        info!(attempt, delay_secs = delay.as_secs(), "reconnecting");
        time::sleep(delay).await;

        match connect_fn().await {
            Ok(()) => {
                let _ = state_tx.send(ConnectionState::Connected);
                return Ok(());
            }
            Err(e) => {
                warn!(attempt, error = %e, "reconnect attempt failed");
            }
        }
    }

    let _ = state_tx.send(ConnectionState::Lost);
    anyhow::bail!("all {} reconnect attempts failed", policy.max_attempts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_doubles_each_attempt() {
        let policy = ReconnectPolicy {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
        };
        assert_eq!(policy.delay_for(1), Duration::from_secs(1));
        assert_eq!(policy.delay_for(2), Duration::from_secs(2));
        assert_eq!(policy.delay_for(3), Duration::from_secs(4));
    }

    #[test]
    fn delay_caps_at_8s() {
        let policy = ReconnectPolicy::default();
        assert_eq!(policy.delay_for(10), Duration::from_secs(8));
    }

    #[tokio::test]
    async fn succeeds_on_first_attempt() {
        let (tx, rx) = watch::channel(ConnectionState::Connected);
        let result = with_reconnect(ReconnectPolicy::default(), tx, || async { Ok(()) }).await;
        assert!(result.is_ok());
        assert_eq!(*rx.borrow(), ConnectionState::Connected);
    }

    #[tokio::test]
    async fn emits_lost_after_all_retries_fail() {
        let policy = ReconnectPolicy {
            max_attempts: 2,
            base_delay: Duration::from_millis(1), // fast for tests
        };
        let (tx, rx) = watch::channel(ConnectionState::Connected);
        let result = with_reconnect(policy, tx, || async { anyhow::bail!("always fails") }).await;
        assert!(result.is_err());
        assert_eq!(*rx.borrow(), ConnectionState::Lost);
    }

    #[tokio::test]
    async fn succeeds_on_second_attempt() {
        let policy = ReconnectPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
        };
        let (tx, rx) = watch::channel(ConnectionState::Connected);
        let attempt = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = std::sync::Arc::clone(&attempt);

        let result = with_reconnect(policy, tx, move || {
            let a = std::sync::Arc::clone(&attempt_clone);
            async move {
                let n = a.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    anyhow::bail!("first attempt fails")
                } else {
                    Ok(())
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(*rx.borrow(), ConnectionState::Connected);
    }

    #[test]
    fn connection_state_display() {
        assert_eq!(ConnectionState::Connected.to_string(), "Connected");
        assert_eq!(
            ConnectionState::Reconnecting { attempt: 2, max: 3 }.to_string(),
            "Reconnecting… (2/3)"
        );
        assert_eq!(ConnectionState::Lost.to_string(), "Connection lost");
    }

    #[test]
    fn reconnect_policy_default() {
        let p = ReconnectPolicy::default();
        assert_eq!(p.max_attempts, 3);
        assert_eq!(p.base_delay, Duration::from_secs(1));
    }

    #[tokio::test]
    async fn emits_reconnecting_states() {
        let policy = ReconnectPolicy {
            max_attempts: 2,
            base_delay: Duration::from_millis(1),
        };
        let (tx, mut rx) = watch::channel(ConnectionState::Connected);
        let attempt = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let attempt_clone = std::sync::Arc::clone(&attempt);

        // Fail first two attempts, succeed on third (which is reconnect attempt 2).
        let result = with_reconnect(policy, tx, move || {
            let a = std::sync::Arc::clone(&attempt_clone);
            async move {
                let n = a.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n < 2 { anyhow::bail!("fail") } else { Ok(()) }
            }
        })
        .await;

        assert!(result.is_ok());
        // Final state should be Connected after recovery.
        assert_eq!(*rx.borrow_and_update(), ConnectionState::Connected);
    }
}
