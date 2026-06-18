//! Persistent telemetry configuration.
//!
//! Persisted as TOML to `<OS config dir>/pairwire/telemetry.toml`.
//! Two fields only (v1 scope):
//!   - `consent` — `None` = not yet decided, `Some(true)` = opted in, `Some(false)` = opted out
//!   - `device_id` — anonymous UUID v4, generated once, never regenerated

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("no OS config directory found")]
    NoConfigDir,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("TOML serialise error: {0}")]
    TomlSerialise(#[from] toml::ser::Error),
}

/// Telemetry configuration persisted to disk.
///
/// # GDPR notes
/// - `device_id` is a random UUID with no relation to any PII.
/// - `consent` drives the process-global gate in [`super::consent`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    /// `None` = user has not yet decided (dialog not shown or dismissed).
    /// `Some(true)` = opted in. `Some(false)` = opted out.
    pub consent: Option<bool>,
    /// Anonymous device identifier. Generated once; never regenerated.
    pub device_id: Uuid,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            consent: None,
            device_id: Uuid::new_v4(),
        }
    }
}

/// Return the path to the TOML config file, or an error if the OS config dir
/// is unavailable.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    let base = dirs::config_dir().ok_or(ConfigError::NoConfigDir)?;
    Ok(base.join("pairwire").join("telemetry.toml"))
}

/// Load config from disk. Returns `TelemetryConfig::default()` if the file
/// does not exist (first run). Propagates I/O or parse errors.
pub fn load() -> Result<TelemetryConfig, ConfigError> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(TelemetryConfig::default());
    }
    let raw = std::fs::read_to_string(&path)?;
    let cfg: TelemetryConfig = toml::from_str(&raw)?;
    Ok(cfg)
}

/// Persist `cfg` to the OS config dir.
///
/// Creates the `pairwire/` directory if it does not exist.
pub fn save(cfg: &TelemetryConfig) -> Result<(), ConfigError> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let toml_str = toml::to_string_pretty(cfg)?;
    std::fs::write(&path, toml_str)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Config round-trips through TOML without data loss.
    #[test]
    fn round_trip_toml() {
        let original = TelemetryConfig {
            consent: Some(true),
            device_id: Uuid::new_v4(),
        };
        let toml_str = toml::to_string_pretty(&original).unwrap();
        let decoded: TelemetryConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(decoded.consent, original.consent);
        assert_eq!(decoded.device_id, original.device_id);
    }

    /// `device_id` survives a save/load cycle via a tempdir.
    #[test]
    fn device_id_persists_across_load_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pairwire").join("telemetry.toml");

        let original = TelemetryConfig {
            consent: Some(false),
            device_id: Uuid::new_v4(),
        };

        // Save manually to the tempdir path (bypassing config_path()).
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let toml_str = toml::to_string_pretty(&original).unwrap();
        std::fs::write(&path, toml_str).unwrap();

        // Load from the same path.
        let raw = std::fs::read_to_string(&path).unwrap();
        let loaded: TelemetryConfig = toml::from_str(&raw).unwrap();

        assert_eq!(
            loaded.device_id, original.device_id,
            "device_id must survive a save/load round-trip"
        );
        assert_eq!(loaded.consent, original.consent);
    }

    /// Default produces a valid UUID (not nil).
    #[test]
    fn default_device_id_is_not_nil() {
        let cfg = TelemetryConfig::default();
        assert_ne!(cfg.device_id, Uuid::nil());
    }

    /// Consent is None on a freshly-defaulted config.
    #[test]
    fn default_consent_is_none() {
        let cfg = TelemetryConfig::default();
        assert_eq!(cfg.consent, None);
    }
}
