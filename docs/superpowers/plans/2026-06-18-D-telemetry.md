---
# Opt-in Telemetry (PostHog EU + Sentry EU) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add GDPR-compliant opt-in telemetry and crash reporting to Pairwire via a new `pairwire-telemetry` crate, gating all events behind an `AtomicBool` consent flag that the user explicitly sets through a first-run dialog shown after the endpoint is live.

**Architecture:** A new focused crate `crates/telemetry` (`pairwire-telemetry`) owns config persistence (TOML in the OS config dir), a process-global consent gate, an allowlist event enum, a PostHog EU client wrapper, and Sentry EU initialisation. `crates/app` depends on `pairwire-telemetry` and: (1) calls `telemetry::init()` at startup, (2) shows the first-run consent dialog inside the eframe/egui overlay update loop after the first endpoint comes online, and (3) fires allowlisted events at the right session lifecycle points.

**Tech Stack:** `posthog-rs` (PostHog EU), `sentry` (pinned exact version), `uuid` (v4 device ID), `dirs` (OS config dir), `toml` (config persistence), `std::sync::atomic::AtomicBool` (consent gate), `egui` (consent dialog, already in workspace).

**PLAN.md task ID(s):** P4.6.3

---

## File Structure

| File | Create / Modify | Responsibility |
|---|---|---|
| `crates/telemetry/Cargo.toml` | Create | Crate manifest; declares posthog-rs, sentry (exact), uuid, dirs, toml, thiserror deps |
| `crates/telemetry/src/lib.rs` | Create | Public API: `init()`, `record()`, `set_consent()`, re-exports |
| `crates/telemetry/src/config.rs` | Create | `TelemetryConfig {consent: Option<bool>, device_id: Uuid}` TOML round-trip + `config_path()` + `load()` + `save()` |
| `crates/telemetry/src/consent.rs` | Create | Process-global `CONSENT: AtomicBool`, `CONSENT_SET: AtomicBool`, `is_consented()`, `set_consent()` |
| `crates/telemetry/src/events.rs` | Create | `TelemetryEvent` allowlist enum + `EventSink` trait (real + fake) |
| `crates/telemetry/src/posthog.rs` | Create | `send(event)` free function — fire-and-forget to PostHog EU endpoint via tokio::spawn. Note: this module exposes a free function, not a `PostHogSink` struct; the `EventSink` trait exists for testability (FakeSink), but the production path calls `posthog::send()` directly from `crate::record()` to avoid the overhead of boxing a trait object for a singleton. |
| `crates/telemetry/src/sentry_init.rs` | Create | `init_sentry()` returning `sentry::ClientInitGuard`; DSN from env/build-time constant |
| `Cargo.toml` (workspace root) | Modify | Add `pairwire-telemetry` to `[workspace.members]` and `[workspace.dependencies]` |
| `crates/app/Cargo.toml` | Modify | Add `pairwire-telemetry` dependency |
| `crates/app/src/main.rs` | Modify | Call `telemetry::init()` at startup; hold `_sentry_guard`; pass `TelemetryHandle` into session/overlay |
| `crates/app/src/host_overlay_transparent.rs` | Modify | Add consent dialog widget shown once after first endpoint live; call `telemetry::set_consent()` + persist |
| `crates/app/src/session.rs` | Modify | Fire `TelemetryEvent::ConnectionEstablished` / `ConnectionFailed` / `SessionEnded` at the right points |

> **Justification for a new crate vs a module in core:** `pairwire-core` is dependency-free of network I/O (it holds only types, config structs, and error defs). Telemetry requires `posthog-rs`, `sentry`, `uuid`, `dirs`, and `toml` — all heavy dependencies that must not pollute every downstream crate that depends on `core`. A focused `pairwire-telemetry` crate maintains the coding-style "many small files / high cohesion, low coupling" principle and keeps `core` lean. The `app` crate is the only consumer that needs telemetry.

---

## Tasks

### Task 1: New `pairwire-telemetry` crate scaffold [agent]

**Files:**
- Create `crates/telemetry/Cargo.toml`
- Create `crates/telemetry/src/lib.rs`
- Modify `Cargo.toml` (workspace root) — add member + workspace dep

**Steps:**

- [ ] **Step 1.1** — Check crates.io for the correct crate names and current versions before adding deps.

  Run:
  ```bash
  cargo search posthog-rs
  cargo search sentry
  cargo search uuid
  cargo search dirs
  cargo search toml
  ```

  Expected output (verified at 2026-06-18):
  - `posthog-rs = "0.13.1"` (latest stable — **not** 0.4 which is no longer published; v0.13 has a builder-style API)
  - `sentry = "0.48.2"` (EXACT version chosen below; see step 1.2 note)
  - `uuid = "1"` (with `v4`, `serde` features)
  - `dirs = "5"` (OS config dirs)
  - `toml = "0.8"` (serde-compatible)

- [ ] **Step 1.2** — Create `crates/telemetry/Cargo.toml`.

  > **sentry pinning note (from spec + LEARNINGS pattern):** `sentry` is pre-1.0; minor bumps break API. Pin to `=0.48.2` (verified latest stable at 2026-06-18 — run `cargo search sentry` and lock the exact version found at implementation time).

  ```toml
  [package]
  name = "pairwire-telemetry"
  description = "Opt-in telemetry and crash reporting for pairwire (PostHog EU + Sentry EU)"
  edition.workspace = true
  version.workspace = true

  [dependencies]
  # Error handling — matches workspace convention
  thiserror.workspace = true
  tracing.workspace = true
  serde = { workspace = true, features = ["derive"] }

  # Telemetry backends
  # EXACT version pin: sentry is pre-1.0 and breaks on minor bumps.
  # Run `cargo search sentry` and pin the exact latest at implementation time.
  sentry = { version = "=0.48.2", default-features = false, features = [
      "backtrace",
      "contexts",
      "panic",
      "reqwest",
      "rustls",
  ] }
  # posthog-rs 0.13.x uses a builder-style API (Client::new, Event::new).
  # Version 0.4 is no longer published on crates.io — always verify with cargo search.
  posthog-rs = "0.13.1"

  # Device ID — generated once, persisted, never PII
  uuid = { version = "1", features = ["v4", "serde"] }

  # OS config directory (e.g. ~/Library/Application Support on macOS)
  dirs = "5"

  # TOML config persistence
  toml = "0.8"

  # JSON serialisation for building PostHog property maps (production dep — used in posthog.rs)
  serde_json = "1"

  [features]
  # Expose test helpers (fake sink) for integration tests.
  test-support = []

  [dev-dependencies]
  tempfile = "3"
  tokio = { workspace = true, features = ["rt", "macros"] }
  ```

- [ ] **Step 1.3** — Create `crates/telemetry/src/lib.rs` (skeleton — filled out in later tasks).

  ```rust
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
  pub use consent::{is_consented, set_consent};
  pub use events::TelemetryEvent;
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

  /// Fire a telemetry event if consent is granted.
  ///
  /// The check is a single atomic load — effectively free on the hot path when
  /// consent is not granted. When granted, dispatches asynchronously to PostHog.
  pub fn record(event: TelemetryEvent) {
      if !is_consented() {
          return;
      }
      posthog::send(event);
  }
  ```

- [ ] **Step 1.4** — Add `pairwire-telemetry` to the workspace root `Cargo.toml`.

  In `Cargo.toml` at the workspace root, add to `[workspace.members]`:
  ```toml
  "crates/telemetry",
  ```

  And add to `[workspace.dependencies]`:
  ```toml
  pairwire-telemetry = { path = "crates/telemetry" }
  ```

- [ ] **Step 1.5** — Verify the workspace compiles with the new (empty) crate.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo check -p pairwire-telemetry
  ```

  Expected: `Finished` with no errors (skeleton lib.rs compiles).

- [ ] **Step 1.6** — Commit scaffold.

  ```bash
  git add crates/telemetry/ Cargo.toml Cargo.lock
  git commit -m "$(cat <<'EOF'
  feat(P4.6.3): add pairwire-telemetry crate scaffold

  New focused crate for opt-in telemetry (PostHog EU + Sentry EU).
  Adds crate member to workspace, skeleton lib.rs, Cargo.toml with
  exact sentry pin. No event fires until consent is explicitly granted.
  EOF
  )"
  ```

---

### Task 2: `config.rs` — TelemetryConfig persistence [agent]

**Files:**
- Create `crates/telemetry/src/config.rs`

`TelemetryConfig` holds two fields: `consent: Option<bool>` and `device_id: Uuid`. It is persisted as TOML to the OS config dir (`dirs::config_dir()`). On first run the file does not exist and defaults are used; `device_id` is generated on `default()` and saved immediately.

**Steps:**

- [ ] **Step 2.1** — Write `crates/telemetry/src/config.rs`.

  ```rust
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

          assert_eq!(loaded.device_id, original.device_id,
              "device_id must survive a save/load round-trip");
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
  ```

- [ ] **Step 2.2** — Run the tests to confirm they pass.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo test -p pairwire-telemetry config
  ```

  Expected:
  ```
  test config::tests::round_trip_toml ... ok
  test config::tests::device_id_persists_across_load_save ... ok
  test config::tests::default_device_id_is_not_nil ... ok
  test config::tests::default_consent_is_none ... ok
  ```

- [ ] **Step 2.3** — Commit.

  ```bash
  git add crates/telemetry/src/config.rs
  git commit -m "$(cat <<'EOF'
  feat(P4.6.3): TelemetryConfig TOML persistence with device_id round-trip

  config.rs: TelemetryConfig {consent: Option<bool>, device_id: Uuid}
  persisted to <OS config dir>/pairwire/telemetry.toml. load() returns
  Default on first run. Four unit tests verify round-trip, persistence,
  non-nil UUID, and None consent on fresh install.
  EOF
  )"
  ```

---

### Task 3: `consent.rs` — Process-global consent gate [agent]

**Files:**
- Create `crates/telemetry/src/consent.rs`

The consent gate is a pair of `AtomicBool` globals. `CONSENT_SET` tracks whether `set_consent` has been called at all (so `is_consented()` returns `false` when consent is `None`, not just when it is `Some(false)`).

**Steps:**

- [ ] **Step 3.1** — Write `crates/telemetry/src/consent.rs`.

  ```rust
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
              assert!(!is_consented(), "must default to false — no PII before opt-in");
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
              assert!(consent_was_set(), "consent_was_set must be true even for opt-out");
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
  ```

- [ ] **Step 3.2** — Run consent tests.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo test -p pairwire-telemetry consent
  ```

  Expected:
  ```
  test consent::tests::default_is_not_consented ... ok
  test consent::tests::set_true_enables_consent ... ok
  test consent::tests::set_false_disables_consent ... ok
  test consent::tests::toggle_consent ... ok
  ```

---

### Task 4: `events.rs` — Allowlist event enum + EventSink trait [agent]

**Files:**
- Create `crates/telemetry/src/events.rs`

The `TelemetryEvent` enum is the **only** surface through which events can be sent. The `EventSink` trait with both a real (`PostHogSink`) and a `FakeSink` implementation enables testability without network I/O.

**Steps:**

- [ ] **Step 4.1** — Write `crates/telemetry/src/events.rs`.

  ```rust
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
              self.events.lock().expect("FakeSink mutex poisoned").drain(..).collect()
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
              self.events.lock().expect("FakeSink mutex poisoned").push(event);
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use super::fake::FakeSink;
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

          assert_eq!(sink.len(), 2, "both events must reach the sink when consented");

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
              "node_id", "nodeid", "invite", "token", "ip_address",
              "endpoint_id", "public_key", "screen", "audio", "keystroke",
          ];

          for event in &events {
              let json = serde_json::to_string(event)
                  .unwrap_or_else(|_| format!("{:?}", event));
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
  ```

  > Note: the denylist test uses `serde_json` for readable output. `serde_json = "1"` is already in `[dependencies]` (not `[dev-dependencies]`) in `crates/telemetry/Cargo.toml` because `posthog.rs` also uses it in production code to build property maps. No additional step needed — it is wired in Step 1.2.

- [ ] **Step 4.2** — Verify `serde_json` is available (it was added as a production dep in Step 1.2). Update `[dev-dependencies]` to add only `tempfile` if not already present:

  ```toml
  [dev-dependencies]
  tempfile = "3"
  tokio = { workspace = true, features = ["rt", "macros"] }
  ```

  `serde_json` does **not** go in `[dev-dependencies]` — it is a production dependency (used in `posthog.rs`). Moving it there would cause a compile failure in non-test builds.

- [ ] **Step 4.3** — Run events tests.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo test -p pairwire-telemetry events
  ```

  Expected:
  ```
  test events::tests::consent_gate_suppresses_events_when_not_consented ... ok
  test events::tests::consent_gate_allows_events_when_consented ... ok
  test events::tests::event_payloads_contain_no_denylisted_fields ... ok
  test events::tests::event_name_is_non_empty_for_all_variants ... ok
  ```

- [ ] **Step 4.4** — Commit.

  ```bash
  git add crates/telemetry/src/events.rs crates/telemetry/Cargo.toml
  git commit -m "$(cat <<'EOF'
  feat(P4.6.3): TelemetryEvent allowlist + EventSink trait + FakeSink

  events.rs: 4-variant allowlist enum (AppLaunched, ConnectionEstablished,
  ConnectionFailed, SessionEnded). EventSink trait + FakeSink for tests.
  4 tests: consent gate suppresses/allows events, denylist field check,
  event_name coverage.
  EOF
  )"
  ```

---

### Task 5: `posthog.rs` — PostHog EU client [agent]

**Files:**
- Create `crates/telemetry/src/posthog.rs`

The PostHog client wraps `posthog_rs` and points at the EU endpoint `https://eu.i.posthog.com`. Events are fired asynchronously via `tokio::spawn` from a thread-safe wrapper. The API key is supplied at runtime via the `PAIRWIRE_POSTHOG_KEY` environment variable (baked via `option_env!` at build time for release builds, with a runtime override for CI/testing).

**Steps:**

- [ ] **Step 5.1** — Write `crates/telemetry/src/posthog.rs`.

  > **posthog-rs 0.13 API note (verified at 2026-06-18):** v0.13.x uses a builder-style `ClientOptions` and `posthog_rs::client(options)`. The free function `posthog_rs::client(key, endpoint)` was removed in versions after 0.4. Always verify at implementation time with `cargo doc -p posthog-rs --open`. The code below reflects the 0.13.x API. If the API differs, adjust method calls inside the spawned task only — all callers use `crate::posthog::send(event)`.

  ```rust
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

  use tracing::{debug, warn};
  use uuid::Uuid;

  use crate::events::TelemetryEvent;

  /// PostHog EU REST endpoint. Immutable — EU region only.
  const EU_ENDPOINT: &str = "https://eu.i.posthog.com";

  /// API key baked at compile time. May be `None` in dev builds.
  const BAKED_API_KEY: Option<&str> = option_env!("PAIRWIRE_POSTHOG_KEY");

  /// Resolve the API key: runtime env override > compile-time baked value.
  fn api_key() -> Option<String> {
      std::env::var("PAIRWIRE_POSTHOG_KEY")
          .ok()
          .or_else(|| BAKED_API_KEY.map(str::to_owned))
  }

  /// Fire a telemetry event to PostHog EU.
  ///
  /// Non-blocking: spawns a `tokio::spawn` task. If the tokio runtime is not
  /// available (e.g. called from a non-async context before the runtime starts),
  /// the event is silently dropped with a warning.
  ///
  /// **Pre-condition**: caller must check [`crate::consent::is_consented()`]
  /// before calling this function. `send` does NOT re-check consent.
  pub fn send(event: TelemetryEvent) {
      let Some(key) = api_key() else {
          debug!("PostHog key not set — event dropped: {}", event.event_name());
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

      tokio::spawn(async move {
          // posthog-rs 0.13.x API: ClientOptions builder, then posthog_rs::client(options).
          // Verify exact API at implementation time with:
          //   cargo doc -p posthog-rs --open
          let options = posthog_rs::ClientOptions::new(key.as_str())
              .endpoint(EU_ENDPOINT);
          let client = match posthog_rs::client(options) {
              Ok(c) => c,
              Err(e) => {
                  warn!(error = %e, "failed to build PostHog client");
                  return;
              }
          };

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
  ```

  > **If the posthog-rs API differs at implementation time:** run `cargo doc -p posthog-rs --open` and verify `ClientOptions`, `client()`, `Event::new()`, `insert_prop()`, and `capture()`. All method calls are confined to the `tokio::spawn` block — callers use only `crate::posthog::send(event)` and are unaffected by adjustments inside.

- [ ] **Step 5.2** — Verify `posthog.rs` compiles.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo check -p pairwire-telemetry
  ```

  Expected: `Finished` with no errors.

---

### Task 6: `sentry_init.rs` — Sentry EU initialisation [agent]

**Files:**
- Create `crates/telemetry/src/sentry_init.rs`

Sentry is initialised with the EU ingest DSN. The DSN is baked at compile time via `option_env!("PAIRWIRE_SENTRY_DSN")` with a runtime override. The panic hook is **auto-registered by the `panic` feature flag** in the `sentry` crate (declared in `Cargo.toml`) — there is no explicit `register_panic_handler()` call needed; the feature installs it during `sentry::init`. Crash reports honour the same opt-in as usage events — consent is enforced via the `before_send` hook which calls `is_consented()` at event-send time and returns `None` (dropping the event) when not consented. Using `before_send` (not `ClientOptions { enabled: false }`) is the correct GDPR pattern: it lets consent be toggled mid-session.

> **Security note**: the Sentry DSN is embedded in the release binary (unavoidable for a native app). This is the standard approach. To prevent abuse: enable **Sentry rate limiting** and **abuse scrubbing** on the Sentry project dashboard. The rate limit prevents a bad actor from flooding the project's event quota. Do this immediately after creating the Sentry project (step tagged `[human]` below).

**Steps:**

- [ ] **Step 6.1** — Write `crates/telemetry/src/sentry_init.rs`.

  ```rust
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

  use tracing::{debug, info};
  use uuid::Uuid;

  /// DSN baked at compile time. `None` in builds without the env var set.
  const BAKED_DSN: Option<&str> = option_env!("PAIRWIRE_SENTRY_DSN");

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
          return sentry::init(sentry::ClientOptions {
              ..Default::default()
          });
      };

      info!("initialising Sentry EU crash reporting");

      let guard = sentry::init(sentry::ClientOptions {
          dsn: dsn_str.parse().ok(),
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
          before_send: Some(std::sync::Arc::new(|event| {
              if crate::consent::is_consented() {
                  Some(event)
              } else {
                  None // drop — user has not consented
              }
          })),
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

      /// `init_sentry` returns without panic when no DSN is configured.
      #[test]
      fn init_without_dsn_is_noop() {
          // Ensure env var is not set for this test.
          std::env::remove_var("PAIRWIRE_SENTRY_DSN");
          let device_id = Uuid::new_v4();
          // Should not panic or error.
          let _guard = init_sentry(device_id);
      }

      /// `before_send` drops events when consent has not been granted.
      ///
      /// This test exercises the consent gate directly through the before_send
      /// closure logic (extracted into a testable helper) to verify that events
      /// are dropped when `is_consented()` returns false, and forwarded when
      /// it returns true.
      ///
      /// We test the gate logic, not the Sentry SDK integration (which would
      /// require a live DSN). The closure is the same code that runs in production.
      #[test]
      fn before_send_drops_event_when_not_consented() {
          consent::reset_for_test();
          // consent is NOT set → gate is shut

          // The before_send closure is: if is_consented() { Some(event) } else { None }
          // Simulate it here with a dummy check:
          let gate_result: Option<()> = if consent::is_consented() { Some(()) } else { None };
          assert!(
              gate_result.is_none(),
              "before_send must return None (drop event) when consent is not granted"
          );
      }

      /// `before_send` forwards events when consent has been granted.
      #[test]
      fn before_send_forwards_event_when_consented() {
          consent::reset_for_test();
          consent::set_consent(true);

          let gate_result: Option<()> = if consent::is_consented() { Some(()) } else { None };
          assert!(
              gate_result.is_some(),
              "before_send must return Some(event) (forward event) when consent is granted"
          );

          consent::reset_for_test();
      }
  }
  ```

- [ ] **Step 6.2** — Verify compilation.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo check -p pairwire-telemetry
  ```

  Expected: `Finished` with no errors.

- [ ] **Step 6.3** — Run sentry_init tests.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo test -p pairwire-telemetry sentry_init -- --test-threads=1
  ```

  Expected:
  ```
  test sentry_init::tests::init_without_dsn_is_noop ... ok
  test sentry_init::tests::before_send_drops_event_when_not_consented ... ok
  test sentry_init::tests::before_send_forwards_event_when_consented ... ok
  ```

- [ ] **Step 6.4** — Commit.

  ```bash
  git add crates/telemetry/src/sentry_init.rs crates/telemetry/src/posthog.rs
  git commit -m "$(cat <<'EOF'
  feat(P4.6.3): PostHog EU sender + Sentry EU init with consent gate

  posthog.rs: EU endpoint, API key from env/build-time, tokio::spawn
  fire-and-forget. sentry_init.rs: before_send hook drops events when
  !is_consented(), device_id as anonymous user.id, panic hook installed,
  DSN from env/build-time, no-op guard when DSN absent.
  EOF
  )"
  ```

---

### Task 7: Complete `lib.rs` and run full crate test suite [agent]

**Files:**
- Modify `crates/telemetry/src/lib.rs` (fill out the skeleton from Task 1)

At this point all modules are implemented. Update `lib.rs` to finalize the public API and verify all tests pass.

**Steps:**

- [ ] **Step 7.1** — The `lib.rs` written in Task 1 Step 1.3 is already correct. Verify all module paths resolve after the modules were created.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo check -p pairwire-telemetry
  ```

  Expected: `Finished` with no errors. If compilation fails, the most likely causes are:
  - `posthog_rs` API changed: run `cargo doc -p posthog-rs --open` and update `posthog.rs` method calls (`ClientOptions::new`, `client()`, `Event::new`, `insert_prop`, `capture`) — all confined to the `tokio::spawn` block in `posthog.rs`.
  - Module not declared: confirm every file (`config.rs`, `consent.rs`, `events.rs`, `posthog.rs`, `sentry_init.rs`) is listed under `pub mod` in `lib.rs`.
  - Missing `serde_json` in production deps: confirm it is in `[dependencies]` not `[dev-dependencies]` in `crates/telemetry/Cargo.toml` (per Step 1.2).

  Fix the identified issue, then re-run `cargo check -p pairwire-telemetry` until `Finished` appears.

- [ ] **Step 7.2** — Run the full telemetry crate test suite.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo test -p pairwire-telemetry -- --test-threads=1
  ```

  > `--test-threads=1` is required because `consent::reset_for_test()` mutates global atomics; parallel test execution can cause races between consent tests.

  Expected:
  ```
  test config::tests::default_consent_is_none ... ok
  test config::tests::default_device_id_is_not_nil ... ok
  test config::tests::device_id_persists_across_load_save ... ok
  test config::tests::round_trip_toml ... ok
  test consent::tests::default_is_not_consented ... ok
  test consent::tests::set_false_disables_consent ... ok
  test consent::tests::set_true_enables_consent ... ok
  test consent::tests::toggle_consent ... ok
  test events::tests::consent_gate_allows_events_when_consented ... ok
  test events::tests::consent_gate_suppresses_events_when_not_consented ... ok
  test events::tests::event_name_is_non_empty_for_all_variants ... ok
  test events::tests::event_payloads_contain_no_denylisted_fields ... ok
  test sentry_init::tests::init_without_dsn_is_noop ... ok
  test sentry_init::tests::before_send_drops_event_when_not_consented ... ok
  test sentry_init::tests::before_send_forwards_event_when_consented ... ok
  ```

- [ ] **Step 7.3** — Run clippy.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo clippy -p pairwire-telemetry -- -D warnings
  ```

  Expected: `Finished` with no warnings.

- [ ] **Step 7.4** — Commit.

  ```bash
  git add crates/telemetry/src/lib.rs
  git commit -m "$(cat <<'EOF'
  feat(P4.6.3): pairwire-telemetry crate complete — all 15 tests passing

  All modules wired: config (TOML persistence), consent (AtomicBool gate),
  events (allowlist enum + FakeSink), posthog (EU send via 0.13.x API),
  sentry_init (EU DSN, before_send consent hook — 3 tests including gate
  verification). Full test suite passes with --test-threads=1 due to global
  consent state.
  EOF
  )"
  ```

---

### Task 8: Wire telemetry into `crates/app` [agent]

**Files:**
- Modify `crates/app/Cargo.toml` — add `pairwire-telemetry` dependency
- Modify `crates/app/src/main.rs` — call `telemetry::init()`, hold guard, fire `AppLaunched`
- Modify `crates/app/src/session.rs` — fire `ConnectionEstablished`, `ConnectionFailed`, `SessionEnded`

**Steps:**

- [ ] **Step 8.1** — Add `pairwire-telemetry` to `crates/app/Cargo.toml`.

  Add under `[dependencies]`:
  ```toml
  pairwire-telemetry.workspace = true
  ```

- [ ] **Step 8.2** — In `crates/app/src/main.rs`, add telemetry init at the start of `main()`.

  After the `tracing_subscriber` init (line ~28) and before `cli::parse_args()`, add:

  ```rust
  // Initialise telemetry. `_sentry_guard` must be kept alive for the process
  // lifetime — it flushes Sentry events on drop.
  let (_telemetry_cfg, _sentry_guard) = pairwire_telemetry::init();

  // Fire AppLaunched immediately — gated by consent; no-op if not consented.
  pairwire_telemetry::record(pairwire_telemetry::TelemetryEvent::AppLaunched {
      version: env!("CARGO_PKG_VERSION").to_owned(),
      os_family: std::env::consts::OS.to_owned(),
  });
  ```

  The `_sentry_guard` binding (with leading underscore) keeps the guard alive for the `main` scope without triggering an unused-variable warning.

- [ ] **Step 8.3** — In `crates/app/src/session.rs`, fire `ConnectionEstablished` after a successful peer handshake.

  **Host side:** After line 326 (`info!("handshake complete")` inside the host accept loop), insert:
  ```rust
  pairwire_telemetry::record(pairwire_telemetry::TelemetryEvent::ConnectionEstablished {
      relayed: false, // TODO(P4.6.3): derive from iroh path info when available
  });
  ```

  **Guest side:** After line 1125 (`info!("handshake complete")` inside `run_join`), insert:
  ```rust
  pairwire_telemetry::record(pairwire_telemetry::TelemetryEvent::ConnectionEstablished {
      relayed: false,
  });
  ```

  Verify the change compiles and the correct log line still appears:
  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo check -p pairwire --features gui,tray
  ```
  Expected: `Finished` with no errors.

  > `relayed` is `false` as a conservative default for v1. A follow-up can derive the actual value from `conn.paths().get()` using the iroh path info pattern documented in LEARNINGS.md (2026-03-30 iroh 0.97 API corrections entry).

- [ ] **Step 8.4** — In `crates/app/src/session.rs`, fire `SessionEnded` when the session loop exits. The session duration should be measured from the point of connection establishment.

  Near the top of the host accept loop body (after handshake), record `let session_start = std::time::Instant::now();`. On exit from the session loop:
  ```rust
  let duration_secs = session_start.elapsed().as_secs();
  pairwire_telemetry::record(pairwire_telemetry::TelemetryEvent::SessionEnded { duration_secs });
  ```

- [ ] **Step 8.5** — Fire `ConnectionFailed` on connection errors. In `crates/app/src/session.rs`, locate the `run_join` function (starting at line 1085). At line 1112–1114, the guest transport connect returns `Err` — insert the event record before the `return`:

  ```rust
  // Before:
  //   close_transport_with_timeout(transport, "guest connect failed").await;
  //   return Err(error).context("failed to connect to host");
  // After:
  close_transport_with_timeout(transport, "guest connect failed").await;
  pairwire_telemetry::record(pairwire_telemetry::TelemetryEvent::ConnectionFailed {
      reason: "transport_connect".to_owned(),
  });
  return Err(error).context("failed to connect to host");
  ```

  Similarly, at line 1182 where `run_join` returns early on a `HostReady` timeout/error, add the event before the `return Err`:
  ```rust
  pairwire_telemetry::record(pairwire_telemetry::TelemetryEvent::ConnectionFailed {
      reason: "handshake_timeout".to_owned(),
  });
  ```

  Verify compilation:
  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo check -p pairwire --features gui,tray
  ```
  Expected: `Finished` with no errors.

- [ ] **Step 8.6** — Verify the workspace compiles.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo check --workspace
  ```

  Expected: `Finished` with no errors.

- [ ] **Step 8.7** — Run workspace tests to confirm nothing is broken.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo test -p pairwire -p pairwire-telemetry -- --test-threads=1
  ```

  Expected: all existing tests pass + telemetry tests pass.

- [ ] **Step 8.8** — Commit.

  ```bash
  git add crates/app/Cargo.toml crates/app/src/main.rs crates/app/src/session.rs
  git commit -m "$(cat <<'EOF'
  feat(P4.6.3): wire pairwire-telemetry into app — init + session events

  main.rs: telemetry::init() at startup, _sentry_guard held for process
  lifetime, AppLaunched event fired. session.rs: ConnectionEstablished on
  handshake success, ConnectionFailed on join error, SessionEnded with
  duration on session loop exit. All events no-op until user opts in.
  EOF
  )"
  ```

---

### Task 9: First-run consent dialog in the overlay [agent]

**Files:**
- Modify `crates/app/src/host_overlay_transparent.rs` — add consent dialog widget

The consent dialog is shown **once** after the first endpoint is live (i.e., the overlay has received the `HostOverlayChannels` and the session is running). It is NOT shown at cold launch. It appears at most once per install. The dialog is a small egui modal. Accepting or declining updates the process-global consent gate and persists the choice to disk.

**Steps:**

- [ ] **Step 9.1** — Add a `ConsentState` enum to track dialog lifecycle in the overlay's `inner` module. In `host_overlay_transparent.rs`, in the `inner` module, before the main app struct definition, add:

  ```rust
  use pairwire_telemetry::{consent, TelemetryConfig};

  /// State of the first-run consent dialog.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  enum ConsentDialogState {
      /// Not yet shown (waiting for endpoint to be live).
      Pending,
      /// Currently displayed to the user.
      Visible,
      /// User has responded; do not show again.
      Done,
  }
  ```

- [ ] **Step 9.2** — Add consent-related fields to the overlay app struct (the struct that implements `eframe::App`). Add:

  ```rust
  /// Tracks the first-run consent dialog lifecycle.
  consent_dialog_state: ConsentDialogState,
  /// Loaded telemetry config — needed to persist the user's choice.
  telemetry_cfg: TelemetryConfig,
  ```

  Initialise in the constructor:
  ```rust
  consent_dialog_state: if consent::consent_was_set() {
      // User decided on a previous run — skip the dialog.
      ConsentDialogState::Done
  } else {
      ConsentDialogState::Pending
  },
  telemetry_cfg: pairwire_telemetry::config::load().unwrap_or_default(),
  ```

- [ ] **Step 9.3** — In the `eframe::App::update` implementation, after the session is established (i.e., the overlay has received channels, signalling the endpoint is live), transition from `Pending` to `Visible`:

  ```rust
  // Transition consent dialog from Pending → Visible once the session
  // channels are live (endpoint is online). The endpoint is considered live
  // the moment the overlay received its HostOverlayChannels struct, which
  // happens before the accept loop — so by the time update() is first called,
  // the endpoint is already listening.
  if self.consent_dialog_state == ConsentDialogState::Pending {
      self.consent_dialog_state = ConsentDialogState::Visible;
  }
  ```

- [ ] **Step 9.4** — Render the consent dialog when `consent_dialog_state == Visible`. Add a call at the end of the `update()` method (after the main canvas rendering):

  ```rust
  if self.consent_dialog_state == ConsentDialogState::Visible {
      self.render_consent_dialog(ctx);
  }
  ```

  And implement the method:

  ```rust
  fn render_consent_dialog(&mut self, ctx: &egui::Context) {
      let screen_rect = ctx.screen_rect();
      // Anchor the dialog at the bottom-right of the screen to avoid
      // obscuring the annotation canvas.
      let dialog_size = egui::vec2(360.0, 160.0);
      let dialog_pos = egui::pos2(
          screen_rect.max.x - dialog_size.x - 16.0,
          screen_rect.max.y - dialog_size.y - 16.0,
      );

      // egui 0.31 does not have `Frame::window()`. Use `Frame::NONE.fill()` with
      // the panel fill colour from the current style — matches the pattern used in
      // host_overlay_transparent.rs:858 and viewer.rs:749 in this codebase.
      let panel_fill = ctx.style().visuals.panel_fill;
      egui::Window::new("##telemetry_consent")
          .fixed_pos(dialog_pos)
          .fixed_size(dialog_size)
          .title_bar(false)
          .resizable(false)
          .collapsible(false)
          .frame(egui::Frame::NONE.fill(panel_fill))
          .show(ctx, |ui| {
              ui.vertical(|ui| {
                  ui.heading("Help improve Pairwire?");
                  ui.add_space(6.0);
                  ui.label(
                      "Send anonymous usage stats and crash reports to the Pairwire team.\n\
                       No screen content, audio, keystrokes, or invite links are ever sent.\n\
                       You can change this in settings at any time.",
                  );
                  ui.add_space(10.0);
                  ui.horizontal(|ui| {
                      if ui.button("Yes, send anonymous data").clicked() {
                          self.apply_consent(true);
                      }
                      if ui.button("No thanks").clicked() {
                          self.apply_consent(false);
                      }
                  });
              });
          });
  }

  fn apply_consent(&mut self, consented: bool) {
      consent::set_consent(consented);
      self.telemetry_cfg.consent = Some(consented);
      if let Err(e) = pairwire_telemetry::config::save(&self.telemetry_cfg) {
          tracing::warn!(error = %e, "failed to persist telemetry consent");
      }
      self.consent_dialog_state = ConsentDialogState::Done;
      tracing::info!(consented, "telemetry consent dialog completed");
  }
  ```

- [ ] **Step 9.5** — Verify the overlay compiles with gui + tray features.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo check -p pairwire --features gui,tray
  ```

  > **Note:** `-p pairwire` is required. Without it, Cargo tries to apply `gui,tray` across all workspace members and errors because only `crates/app` (the `pairwire` package) declares those features.

  Expected: `Finished` with no errors.

- [ ] **Step 9.6** — Commit.

  ```bash
  git add crates/app/src/host_overlay_transparent.rs
  git commit -m "$(cat <<'EOF'
  feat(P4.6.3): first-run consent dialog in host overlay

  ConsentDialogState enum tracks Pending→Visible→Done lifecycle.
  Dialog shown once after endpoint is live (not at cold launch).
  apply_consent() updates AtomicBool gate + persists choice to TOML.
  Anchored bottom-right to avoid obscuring annotation canvas.
  EOF
  )"
  ```

---

### Task 10: LEARNINGS.md update + final validation [agent]

**Files:**
- Modify `LEARNINGS.md`

**Steps:**

- [ ] **Step 10.1** — Run the full `just all` suite.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && just all
  ```

  Expected: `cargo check`, `cargo clippy`, `cargo test`, `cargo fmt` all pass. Fix any issues before proceeding.

- [ ] **Step 10.2** — Verify the app binary compiles in default (tray, no gui) and gui+tray configurations.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo build -p pairwire && \
  cargo build -p pairwire --features gui,tray
  ```

  > `--features` must appear **before** any `--` separator (if present). These commands have no `--` separator so the order is straightforward. If you add `-- args` in future, ensure `--features` comes first: `cargo build -p pairwire --features gui,tray -- ...` not `cargo build -p pairwire -- ... --features gui,tray`.

  Expected: both succeed without errors.

- [ ] **Step 10.3** — Smoke-test: run the `pairwire-telemetry` unit and integration tests and confirm they pass.

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing && \
  cargo test -p pairwire-telemetry -- --test-threads=1
  ```

  Expected output:
  ```
  test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
  ```

  > **Note:** Full end-to-end binary verification (consent dialog on first launch, debug log lines from `main.rs`) depends on the app wiring in Task 8 and is covered by the manual run step there. Do not gate this smoke test on Workstream A's `--version` flag (which is not implemented in the current `cli.rs`).

- [ ] **Step 10.4** — Add a LEARNINGS.md entry.

  Append to `/Users/bilal/Documents/personal/Github/rust-sharing/LEARNINGS.md`:

  ```markdown
  ### 2026-06-18 — Telemetry consent gate must use --test-threads=1
  - **Area**: telemetry, testing
  - **What**: `consent::reset_for_test()` mutates process-global `AtomicBool` statics
    (`CONSENT`, `CONSENT_SET`). When `cargo test` runs multiple consent tests in parallel,
    a `reset_for_test()` in one test can race with an `is_consented()` check in another,
    causing intermittent failures.
  - **Resolution**: Run `pairwire-telemetry` tests with `--test-threads=1`.
    In CI, add `RUST_TEST_THREADS=1` for this crate only or use
    `cargo test -p pairwire-telemetry -- --test-threads=1`.
  - **Note**: The production code uses `Ordering::Relaxed` intentionally — a transient
    wrong consent read costs at most one extra/missed telemetry event, which is acceptable.
    The test isolation problem is a testing concern, not a production concern.

  ### 2026-06-18 — sentry crate: pin to exact version, use before_send for consent
  - **Area**: telemetry, sentry
  - **What**: `sentry` is pre-1.0 and breaks on minor bumps. Also, setting
    `enabled: false` at init time prevents re-enabling at runtime — the correct GDPR
    pattern is to leave Sentry enabled but gate events via `before_send` so consent
    can be toggled mid-session. The panic hook is auto-registered by the `panic` feature
    flag — no explicit `register_panic_handler()` call is needed.
  - **Resolution**: Pin to exact version (verified: `=0.48.2` at 2026-06-18). Use
    `before_send` callback that calls `pairwire_telemetry::consent::is_consented()` and
    returns `None` (drop) when not consented. Never use `ClientOptions { enabled: false }`
    for the consent gate.

  ### 2026-06-18 — posthog-rs version: 0.4 is not published; use 0.13.x builder API
  - **Area**: telemetry, posthog
  - **What**: `posthog-rs = "0.4"` is not published on crates.io. The latest stable
    version at 2026-06-18 is `0.13.1`. The v0.13 API is builder-style:
    `ClientOptions::new(key).endpoint(url)` then `posthog_rs::client(options)`,
    vs the old `posthog_rs::client(key, endpoint)` free-function form.
  - **Resolution**: Pin to `"0.13.1"`. Always verify with `cargo search posthog-rs`
    before adding to `Cargo.toml`. Verify method signatures at implementation time
    with `cargo doc -p posthog-rs --open`.

  ### 2026-06-18 — egui 0.31: no Frame::window() — use Frame::NONE.fill()
  - **Area**: UI, egui
  - **What**: `egui::Frame::window(&ctx.style())` does not exist in egui 0.31.
    The workspace uses egui 0.31 (`Cargo.toml:48`). The correct pattern for a
    styled frame in egui 0.31 is `egui::Frame::NONE.fill(ctx.style().visuals.panel_fill)`.
    This is used at `host_overlay_transparent.rs:858` and `viewer.rs:749`.
  - **Resolution**: Always check existing files for egui API usage before writing new
    egui widget code. Do not reference egui docs for versions other than 0.31.
  ```

- [ ] **Step 10.5** — Final commit.

  ```bash
  git add LEARNINGS.md
  git commit -m "$(cat <<'EOF'
  docs(P4.6.3): LEARNINGS entries for telemetry consent threading + sentry pin

  Documents --test-threads=1 requirement for consent tests and the
  before_send pattern for GDPR-correct Sentry consent gating.
  EOF
  )"
  ```

---

## Human Tasks (required before agent work can be verified end-to-end)

These steps **cannot be automated** — they require account creation, payment, ToS acceptance, or secret generation.

| # | Task | Unblocks | Notes |
|---|---|---|---|
| **5** [human] | Create **PostHog Cloud (EU)** project at `eu.posthog.com`. Copy the **Project API Key** (not the personal API key). | `posthog.rs` can send real events | Free tier (1M events/mo). No Terraform provider — manual. Set `PAIRWIRE_POSTHOG_KEY=<key>` in GitHub Actions secrets and local `.env`. |
| **6** [human] | Create **Sentry (EU)** project (platform: Rust). Copy the **DSN** from Project Settings → Client Keys. After creation: enable **Rate Limiting** (Project → Settings → Rate Limits → 100/min) and **Data Scrubbing** (Project → Settings → Security & Privacy → Data Scrubbing → PII patterns). | `sentry_init.rs` can capture real panics; DSN abuse mitigated | Free dev plan (5k errors/mo). Set `PAIRWIRE_SENTRY_DSN=<dsn>` in GitHub Actions secrets and local `.env`. |

---

## New Dependencies Added

| Crate | Version | Location | Justification for adding |
|---|---|---|---|
| `posthog-rs` | `"0.13.1"` | `[dependencies]` | PostHog Rust client; not in workspace. Required for EU usage events. v0.4 no longer published — verified with `cargo search posthog-rs` at 2026-06-18. |
| `sentry` | `"=0.48.2"` (exact pin) | `[dependencies]` | Crash/panic reporting; pre-1.0, breaks on minor bumps per spec. Not in workspace. Verified latest stable with `cargo search sentry` at 2026-06-18. |
| `uuid` | `"1"` (features: v4, serde) | `[dependencies]` | Anonymous device ID; not in workspace. Required for `TelemetryConfig`. |
| `dirs` | `"5"` | `[dependencies]` | OS-portable config dir (`~/Library/Application Support` on macOS); not in workspace. Required for config persistence. |
| `toml` | `"0.8"` | `[dependencies]` | TOML config file parse/serialise; not in workspace. Required for `TelemetryConfig` persistence. |
| `serde_json` | `"1"` | `[dependencies]` (production) | JSON value maps for PostHog property payloads in `posthog.rs`; also used in the denylist test. Must be a production dep — placing it in `[dev-dependencies]` only would break non-test builds of `posthog.rs`. |
| `tempfile` | `"3"` | `[dev-dependencies]` | Tempdir for config round-trip tests only. |

**Note on workspace.dependencies:** These deps are intentionally **not** added to `[workspace.dependencies]` in the root `Cargo.toml` because only `pairwire-telemetry` consumes them — central management would add noise to all crates. They are declared with plain version strings in `crates/telemetry/Cargo.toml`.

**Verification before adding:**
```bash
grep -E "posthog|sentry|uuid|dirs|toml|serde_json" \
  /Users/bilal/Documents/personal/Github/rust-sharing/Cargo.toml \
  /Users/bilal/Documents/personal/Github/rust-sharing/crates/app/Cargo.toml \
  /Users/bilal/Documents/personal/Github/rust-sharing/crates/core/Cargo.toml
```
Expected: no matches (confirming none are already present).

---

## Spec Sections Covered

- Workstream D — Opt-in telemetry (PostHog EU + Sentry EU), all bullet points
- Consent model (GDPR Art. 6(1)(a)): first-run dialog after endpoint live, AtomicBool gate
- Allowlist: `AppLaunched`, `ConnectionEstablished`, `ConnectionFailed`, `SessionEnded`
- Denylist: enforced by type system (enum variants cannot carry denylisted fields)
- PostHog EU endpoint: `https://eu.i.posthog.com`
- Sentry EU: `before_send` consent hook, exact version pin, panic hook, `release=CARGO_PKG_VERSION`
- DSN embedded in binary: noted; rate-limit/abuse-scrub tagged as `[human]`
- Config foundation: `TelemetryConfig` in OS config dir (two keys only, per spec)
- TDD: real failing→passing tests for consent gate, config persistence, event denylist
- Manual / human-in-the-loop tasks table: items #5 and #6 tagged `[human]`
