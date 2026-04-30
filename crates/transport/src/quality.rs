//! Quality adaptation: monitor QUIC connection stats and emit encoder config changes.
//!
//! Polls RTT and packet loss every second. Applies adaptation rules:
//!
//! | Condition              | Action                              |
//! |------------------------|-------------------------------------|
//! | loss > 15%             | 15 fps, 720p, longer keyframe interval |
//! | loss > 5%              | 720p                                |
//! | RTT > 200ms            | 15 fps, longer keyframe interval    |
//! | All clear (3 samples)  | Restore configured defaults         |
//!
//! The adapted config is sent on a `watch` channel so the encode thread can
//! reconfigure without restarting.

use std::{sync::Arc, time::Duration};

use iroh::endpoint::Connection;
use pairwire_core::config::CaptureConfig;
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
    time,
};
use tracing::{debug, info, instrument};

/// Poll interval for connection stats.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Number of consecutive "all clear" samples before restoring full quality.
const RECOVERY_SAMPLES: u32 = 3;

/// Quality tier derived from connection health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityTier {
    /// Full quality — use the session's configured defaults.
    Full,
    /// Reduced resolution (720p) due to moderate loss.
    Reduced,
    /// Minimum quality — low fps + 720p due to severe loss or high RTT.
    Degraded,
}

impl QualityTier {
    fn apply(self, base: &CaptureConfig) -> CaptureConfig {
        match self {
            QualityTier::Full => base.clone(),
            QualityTier::Reduced => CaptureConfig {
                width: base.width.min(1280),
                height: base.height.min(720),
                ..base.clone()
            },
            QualityTier::Degraded => CaptureConfig {
                width: base.width.min(1280),
                height: base.height.min(720),
                fps: base.fps.min(15),
                keyframe_interval_secs: base.keyframe_interval_secs.max(4),
                ..base.clone()
            },
        }
    }
}

/// Snapshot of connection health metrics.
#[derive(Debug, Clone, Default)]
pub struct ConnectionHealth {
    pub rtt_ms: u32,
    /// Packet loss rate in per-mille (0–1000, where 1000 = 100%).
    pub loss_per_mille: u16,
}

/// Loss percentage above which quality is fully degraded (fps + resolution drop).
const LOSS_DEGRADED_PCT: f32 = 15.0;
/// Loss percentage above which quality is reduced (resolution drop only).
const LOSS_REDUCED_PCT: f32 = 5.0;
/// RTT threshold (ms) above which quality is reduced.
const RTT_REDUCED_MS: u32 = 200;

impl ConnectionHealth {
    fn tier(&self) -> QualityTier {
        let loss_pct = f32::from(self.loss_per_mille) / 10.0;
        if loss_pct > LOSS_DEGRADED_PCT {
            QualityTier::Degraded
        } else if loss_pct > LOSS_REDUCED_PCT || self.rtt_ms > RTT_REDUCED_MS {
            QualityTier::Reduced
        } else {
            QualityTier::Full
        }
    }
}

/// Handle for the quality monitor task.
pub struct QualityMonitorHandle {
    stop_tx: oneshot::Sender<()>,
    join: JoinHandle<()>,
}

impl QualityMonitorHandle {
    pub async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.join.await;
    }
}

/// Monitors QUIC connection health and emits adapted `CaptureConfig` values.
pub struct QualityMonitor {
    conn: Arc<Connection>,
    base_config: CaptureConfig,
}

impl QualityMonitor {
    #[must_use]
    pub fn new(conn: Arc<Connection>, base_config: CaptureConfig) -> Self {
        Self { conn, base_config }
    }

    /// Spawn the monitor.
    ///
    /// Returns a handle, a `watch::Receiver<CaptureConfig>` (adapted config for the
    /// encode thread), and a `watch::Receiver<QualityTier>` (current tier for the UI).
    #[instrument(skip(self))]
    pub fn spawn(
        self,
    ) -> (
        QualityMonitorHandle,
        watch::Receiver<CaptureConfig>,
        watch::Receiver<QualityTier>,
    ) {
        let (config_tx, config_rx) = watch::channel(self.base_config.clone());
        let (tier_tx, tier_rx) = watch::channel(QualityTier::Full);
        let (stop_tx, mut stop_rx) = oneshot::channel();

        let join = tokio::spawn(async move {
            info!("quality monitor started");
            let mut interval = time::interval(POLL_INTERVAL);
            let mut recovery_count: u32 = 0;
            let mut current_tier = QualityTier::Full;

            loop {
                tokio::select! {
                    biased;
                    _ = &mut stop_rx => break,
                    _ = interval.tick() => {
                        let health = sample_health(&self.conn);
                        let new_tier = health.tier();

                        debug!(
                            rtt_ms = health.rtt_ms,
                            loss_per_mille = health.loss_per_mille,
                            tier = ?new_tier,
                            "connection health sample"
                        );

                        // Require RECOVERY_SAMPLES consecutive full-quality samples
                        // before upgrading, to avoid oscillation.
                        let effective_tier = if new_tier == QualityTier::Full {
                            recovery_count += 1;
                            if recovery_count >= RECOVERY_SAMPLES {
                                QualityTier::Full
                            } else {
                                current_tier
                            }
                        } else {
                            recovery_count = 0;
                            new_tier
                        };

                        if effective_tier != current_tier {
                            current_tier = effective_tier;
                            let adapted = effective_tier.apply(&self.base_config);
                            info!(
                                tier = ?effective_tier,
                                fps = adapted.fps,
                                width = adapted.width,
                                "quality tier changed"
                            );
                            let _ = config_tx.send(adapted);
                            let _ = tier_tx.send(effective_tier);
                        }
                    }
                }
            }
            info!("quality monitor stopped");
        });

        (QualityMonitorHandle { stop_tx, join }, config_rx, tier_rx)
    }
}

fn sample_health(conn: &Connection) -> ConnectionHealth {
    use n0_watcher::Watcher;

    // RTT: read from the first active path via PathWatcher.
    let path_list = conn.paths().get();
    #[allow(clippy::cast_possible_truncation)] // RTT in ms fits in u32 (max ~49 days)
    let rtt_ms = path_list
        .iter()
        .find_map(|p: &iroh::endpoint::PathInfo| p.rtt())
        .map_or(0, |d| d.as_millis() as u32);

    // Loss proxy: RESET_STREAM frames sent relative to total stream frames.
    // Not a true loss rate, but a useful signal for congestion.
    let stats = conn.stats();
    let resets = stats.frame_tx.reset_stream;
    let streams = stats.frame_tx.stream.saturating_add(resets);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    // loss_per_mille is clamped to 1000.0 before cast; always non-negative
    let loss_per_mille = if streams > 0 {
        ((resets as f64 / streams as f64) * 1000.0).min(1000.0) as u16
    } else {
        0
    };

    ConnectionHealth {
        rtt_ms,
        loss_per_mille,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pairwire_core::config::CaptureConfig;

    fn base() -> CaptureConfig {
        CaptureConfig {
            fps: 30,
            width: 1920,
            height: 1080,
            keyframe_interval_secs: 2,
            ..Default::default()
        }
    }

    #[test]
    fn full_tier_no_change() {
        let h = ConnectionHealth {
            rtt_ms: 20,
            loss_per_mille: 0,
        };
        assert_eq!(h.tier(), QualityTier::Full);
        let cfg = QualityTier::Full.apply(&base());
        assert_eq!(cfg.fps, 30);
        assert_eq!(cfg.width, 1920);
    }

    #[test]
    fn moderate_loss_reduces_resolution() {
        let h = ConnectionHealth {
            rtt_ms: 50,
            loss_per_mille: 80,
        }; // 8%
        assert_eq!(h.tier(), QualityTier::Reduced);
        let cfg = QualityTier::Reduced.apply(&base());
        assert_eq!(cfg.width, 1280);
        assert_eq!(cfg.height, 720);
        assert_eq!(cfg.fps, 30); // fps unchanged
    }

    #[test]
    fn severe_loss_degrades_fully() {
        let h = ConnectionHealth {
            rtt_ms: 50,
            loss_per_mille: 200,
        }; // 20%
        assert_eq!(h.tier(), QualityTier::Degraded);
        let cfg = QualityTier::Degraded.apply(&base());
        assert_eq!(cfg.width, 1280);
        assert_eq!(cfg.fps, 15);
        assert_eq!(cfg.keyframe_interval_secs, 4);
    }

    #[test]
    fn high_rtt_degrades() {
        let h = ConnectionHealth {
            rtt_ms: 250,
            loss_per_mille: 0,
        };
        assert_eq!(h.tier(), QualityTier::Reduced);
    }

    #[test]
    fn very_high_rtt_with_loss_degrades() {
        let h = ConnectionHealth {
            rtt_ms: 250,
            loss_per_mille: 200,
        };
        assert_eq!(h.tier(), QualityTier::Degraded);
    }

    // --- Boundary threshold tests ---

    #[test]
    fn loss_at_exactly_5_percent_is_full() {
        // 5.0% = 50 per-mille, threshold is > 5% so exactly at is Full
        let h = ConnectionHealth {
            rtt_ms: 0,
            loss_per_mille: 50,
        };
        assert_eq!(h.tier(), QualityTier::Full);
    }

    #[test]
    fn loss_just_above_5_percent_is_reduced() {
        let h = ConnectionHealth {
            rtt_ms: 0,
            loss_per_mille: 51,
        };
        assert_eq!(h.tier(), QualityTier::Reduced);
    }

    #[test]
    fn loss_at_exactly_15_percent_is_reduced() {
        // 15.0% = 150 per-mille, threshold is > 15% so exactly at is Reduced
        let h = ConnectionHealth {
            rtt_ms: 0,
            loss_per_mille: 150,
        };
        assert_eq!(h.tier(), QualityTier::Reduced);
    }

    #[test]
    fn loss_just_above_15_percent_is_degraded() {
        let h = ConnectionHealth {
            rtt_ms: 0,
            loss_per_mille: 151,
        };
        assert_eq!(h.tier(), QualityTier::Degraded);
    }

    #[test]
    fn rtt_at_exactly_200ms_is_full() {
        let h = ConnectionHealth {
            rtt_ms: 200,
            loss_per_mille: 0,
        };
        assert_eq!(h.tier(), QualityTier::Full);
    }

    #[test]
    fn rtt_at_201ms_is_reduced() {
        let h = ConnectionHealth {
            rtt_ms: 201,
            loss_per_mille: 0,
        };
        assert_eq!(h.tier(), QualityTier::Reduced);
    }

    #[test]
    fn zero_health_is_full() {
        let h = ConnectionHealth::default();
        assert_eq!(h.tier(), QualityTier::Full);
    }

    #[test]
    fn max_loss_is_degraded() {
        let h = ConnectionHealth {
            rtt_ms: 0,
            loss_per_mille: 1000,
        };
        assert_eq!(h.tier(), QualityTier::Degraded);
    }

    // --- Tier application edge cases ---

    #[test]
    fn reduced_tier_preserves_small_resolution() {
        // If base is already 720p, Reduced shouldn't upscale
        let small = CaptureConfig {
            width: 640,
            height: 480,
            fps: 30,
            ..Default::default()
        };
        let cfg = QualityTier::Reduced.apply(&small);
        assert_eq!(cfg.width, 640);
        assert_eq!(cfg.height, 480);
    }

    #[test]
    fn degraded_tier_preserves_low_fps() {
        let slow = CaptureConfig {
            fps: 10,
            width: 1920,
            height: 1080,
            ..Default::default()
        };
        let cfg = QualityTier::Degraded.apply(&slow);
        assert_eq!(cfg.fps, 10); // already below 15, don't change
    }
}
