---
# Workstream E — Minimal Launch Update Version-Check Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On launch, fire a non-blocking background tokio task that GETs `https://pairwire.dev/version.json`, compares the remote version to the running binary's `CARGO_PKG_VERSION`, and — if a newer version exists — surfaces a one-time "Update available → download" notification via the existing overlay/tray banner surface without ever blocking startup or panicking.

**Architecture:** A new module `crates/app/src/update_check.rs` owns all update-check logic: a pure `compare_versions` function, a `VersionInfo` serde type matching the `/version.json` contract, and a `spawn_update_check` function that fires a tokio task and returns a `watch::Receiver<Option<UpdateAvailable>>`. `main.rs` calls `spawn_update_check` before any blocking call and threads the receiver into the overlay and viewer surfaces. The overlay and viewer each read the receiver in their `update()` loops and render a dismissible banner pill when `Some(UpdateAvailable)` is present. The HTTP client is `reqwest` 0.12, which is **already in `Cargo.lock`** as a transitive dependency of iroh 0.97 — no new crate is introduced; we add it as an explicit `[dependencies]` entry in `crates/app/Cargo.toml` only. Semver comparison is done by a hand-rolled `compare_versions(current: &str, remote: &str) -> Ordering` function (major.minor.patch only) — `semver` 1.0 is also in `Cargo.lock` transitively but is unjustified here because the comparison is trivial and adding it would pull it into the explicit dep set unnecessarily.

**Tech Stack:** Rust 2024 edition, tokio async task, reqwest 0.12 (already in workspace lock), serde + serde_json (already in workspace lock), `watch::channel` for notification delivery, egui painter API for the banner pill (same pattern as `render_session_banner`).

**PLAN.md task ID(s):** P4.6.4

---

## File Structure

| File | Action | Single responsibility |
|---|---|---|
| `crates/app/src/update_check.rs` | **Create** | Pure version-compare fn + `VersionInfo` deserialization + `spawn_update_check` tokio task |
| `crates/app/Cargo.toml` | **Modify** | Add `reqwest` and `serde_json` as explicit workspace deps (already in lock) |
| `Cargo.toml` (workspace root) | **Modify** | Add `reqwest` and `serde_json` to `[workspace.dependencies]` |
| `crates/app/src/main.rs` | **Modify** | Call `spawn_update_check` at startup; thread `update_rx` into overlay and viewer |
| `crates/app/src/host_overlay_transparent.rs` | **Modify** | Accept `update_rx` field; render update banner in `update()` |
| `crates/app/src/viewer.rs` | **Modify** | Accept `update_rx` in `ViewerApp::new` and `run_viewer`; render update banner |

---

## Tasks

---

### Task 1: Establish workspace dependency entries for reqwest and serde_json

**Files:**
- Modify `Cargo.toml` (workspace root) — `[workspace.dependencies]` block
- Modify `crates/app/Cargo.toml` — `[dependencies]` block

`reqwest` 0.12.28 and `serde_json` 1.0.149 are **already in `Cargo.lock`** (pulled transitively by iroh). Adding them explicitly here causes zero version resolution changes — cargo will simply pin to the already-resolved versions.

The `rustls-tls` feature flag: the existing transitive resolution of reqwest 0.12.28 already uses `rustls` (not native-tls). Adding `rustls-tls` explicitly activates the same backend. If the feature resolves to a different sub-feature variant than what iroh currently activates (e.g. `rustls-tls-webpki-roots` vs `rustls-tls-native-roots`), the agent must confirm via `cargo tree -e features -i reqwest` (inverse query — valid for a transitive/direct dep, unlike `-p <pkg> --features` which errors for non-workspace members) that no new compile units are introduced. If new units appear, switch to `default-features = false, features = ["json"]` and rely on iroh's existing TLS activation.

- [ ] [agent] **Step 1.1** — Open `/Users/bilal/Documents/personal/Github/rust-sharing/Cargo.toml`. In `[workspace.dependencies]`, after the `# Hex encoding` block, add:

```toml
# HTTP client (already pulled transitively by iroh; explicit for update-check)
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }

# JSON (already pulled transitively by reqwest; explicit for version.json parsing)
serde_json = "1"
```

- [ ] [agent] **Step 1.2** — Open `/Users/bilal/Documents/personal/Github/rust-sharing/crates/app/Cargo.toml`. In `[dependencies]`, after the `rand = "0.9"` line, add:

```toml
reqwest = { workspace = true }
serde_json = { workspace = true }
serde = { workspace = true }
```

`serde` is a workspace dependency (root Cargo.toml line 29: `serde = { version = "1", features = ["derive"] }`). `update_check.rs` uses `use serde::Deserialize` and `#[derive(Deserialize)]` — without this entry, `crates/app` has no direct `serde` dep and the derive macro will fail to compile. `reqwest` is present in `Cargo.lock` transitively (via iroh) but is NOT a direct dep of `crates/app`; it must be declared directly here to use it. The exact Cargo.toml lines to add:

```toml
# Explicitly declared (already resolved in Cargo.lock via iroh):
reqwest = { workspace = true }
serde_json = { workspace = true }
# serde is a workspace dep (root Cargo.toml:29); needed for #[derive(Deserialize)] in update_check.rs:
serde = { workspace = true }
```

- [ ] [agent] **Step 1.3** — Verify no version conflict and no new compile units:

```bash
cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo tree -p pairwire | grep reqwest
```

Expected output (reqwest already in transitive dep tree via iroh; no new version should appear):
```
│   └── reqwest v0.12.28
```

Then run:

```bash
just check
```

Expected: compiles cleanly; no version-resolution errors.

- [ ] [agent] **Step 1.4 — Commit**

```
git commit -m "feat(P4.6.4): add reqwest + serde_json as explicit workspace deps for update check"
```

---

### Task 2: Implement `update_check.rs` with TDD

**Files:**
- Create `crates/app/src/update_check.rs`

#### 2a. TDD — pure `compare_versions` function

The function signature:

```rust
/// Returns `Ordering::Less` if `current < remote` (update available),
/// `Ordering::Equal` if equal, `Ordering::Greater` if current is ahead.
/// Returns `None` if either string is not a valid `major.minor.patch` triple.
pub fn compare_versions(current: &str, remote: &str) -> Option<std::cmp::Ordering>
```

**Step 2.1** — [agent] Write the **tests only** first. Create `crates/app/src/update_check.rs` containing ONLY the `#[cfg(test)] mod tests` block plus the minimal skeleton (type stubs with `todo!()`) needed for the file to compile. The `compare_versions` body must be `todo!()` — NOT the real implementation. This ensures the test run in Step 2.2 genuinely fails.

```rust
//! Minimal launch update version-check.
//!
//! On startup, `spawn_update_check` fires a non-blocking tokio task that GETs
//! `https://pairwire.dev/version.json`, compares the remote version to the
//! running binary's `CARGO_PKG_VERSION`, and publishes a `UpdateAvailable`
//! value to a `watch` channel if a newer version exists.
//!
//! All network and parse errors are logged at `debug!` level and silently
//! ignored — they must never block startup or panic.

use std::cmp::Ordering;
use serde::Deserialize;
use tokio::sync::watch;
use tracing::debug;

/// The shape of `https://pairwire.dev/version.json` as defined in Workstream C.
///
/// ```json
/// { "version": "0.2.0", "url": "https://pairwire.dev/download", "notes": "Bug fixes" }
/// ```
#[derive(Debug, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub url: String,
    pub notes: String,
}

/// Payload published to the update watch channel when a newer version is found.
#[derive(Debug, Clone)]
pub struct UpdateAvailable {
    /// The remote version string, e.g. `"0.2.0"`.
    pub version: String,
    /// The download URL from `version.json`.
    pub url: String,
    /// Optional release notes.
    pub notes: String,
}

/// Compare two semver-style `"major.minor.patch"` version strings.
///
/// Returns `Some(Ordering)`:
/// - `Less`    → `current` is older than `remote` (update available)
/// - `Equal`   → same version
/// - `Greater` → current is newer than remote (dev/pre-release build)
///
/// Returns `None` if either string is not a valid `MAJOR.MINOR.PATCH` triple
/// of non-negative integers. Callers treat `None` as "no update, no panic".
pub fn compare_versions(_current: &str, _remote: &str) -> Option<Ordering> {
    todo!("implement in Step 2.3")
}

/// Spawn a non-blocking background tokio task that GETs `https://pairwire.dev/version.json`,
/// parses it, and publishes an `UpdateAvailable` to the returned watch channel if the remote
/// version is newer than `current_version`.
pub fn spawn_update_check(current_version: &'static str) -> watch::Receiver<Option<UpdateAvailable>> {
    let (_tx, rx) = watch::channel(None::<UpdateAvailable>);
    // TODO: implement in Step 2.3
    let _ = (current_version, debug!("stub"));
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── compare_versions table tests ─────────────────────────────────────────────

    #[test]
    fn older_current_returns_less() {
        assert_eq!(compare_versions("0.1.0", "0.2.0"), Some(Ordering::Less));
    }

    #[test]
    fn equal_versions_returns_equal() {
        assert_eq!(compare_versions("0.1.0", "0.1.0"), Some(Ordering::Equal));
    }

    #[test]
    fn newer_current_returns_greater() {
        assert_eq!(compare_versions("0.2.0", "0.1.0"), Some(Ordering::Greater));
    }

    #[test]
    fn major_version_bump_detected() {
        assert_eq!(compare_versions("0.9.9", "1.0.0"), Some(Ordering::Less));
    }

    #[test]
    fn patch_version_bump_detected() {
        assert_eq!(compare_versions("0.1.0", "0.1.1"), Some(Ordering::Less));
    }

    #[test]
    fn malformed_current_returns_none() {
        assert_eq!(compare_versions("not-a-version", "0.2.0"), None);
    }

    #[test]
    fn malformed_remote_returns_none() {
        assert_eq!(compare_versions("0.1.0", "not-a-version"), None);
    }

    #[test]
    fn both_malformed_returns_none() {
        assert_eq!(compare_versions("bad", "also-bad"), None);
    }

    #[test]
    fn empty_string_returns_none() {
        assert_eq!(compare_versions("", "0.1.0"), None);
    }

    #[test]
    fn too_many_components_returns_none() {
        // Four components is not a valid major.minor.patch triple.
        assert_eq!(compare_versions("0.1.0", "0.1.0.1"), None);
    }

    #[test]
    fn two_components_returns_none() {
        assert_eq!(compare_versions("0.1", "0.2.0"), None);
    }

    #[test]
    fn leading_whitespace_is_trimmed() {
        // trim() is applied before parsing.
        assert_eq!(compare_versions("0.1.0", " 0.2.0"), Some(Ordering::Less));
    }

    // ── VersionInfo deserialization tests ────────────────────────────────────────

    #[test]
    fn version_info_deserializes_valid_json() {
        let json = r#"{"version":"0.2.0","url":"https://pairwire.dev/download","notes":"Bug fixes"}"#;
        let info: VersionInfo = serde_json::from_str(json).expect("valid JSON must parse");
        assert_eq!(info.version, "0.2.0");
        assert_eq!(info.url, "https://pairwire.dev/download");
        assert_eq!(info.notes, "Bug fixes");
    }

    #[test]
    fn version_info_fails_on_missing_field() {
        let json = r#"{"version":"0.2.0","url":"https://pairwire.dev/download"}"#;
        // `notes` is missing — serde must return an error.
        let result = serde_json::from_str::<VersionInfo>(json);
        assert!(result.is_err(), "missing `notes` field must be a parse error");
    }

    #[test]
    fn version_info_fails_on_empty_input() {
        let result = serde_json::from_str::<VersionInfo>("");
        assert!(result.is_err());
    }

    #[test]
    fn version_info_fails_on_wrong_type() {
        let json = r#"{"version":42,"url":"https://pairwire.dev","notes":""}"#;
        // `version` is a number, not a string — must fail.
        let result = serde_json::from_str::<VersionInfo>(json);
        assert!(result.is_err());
    }
}
```

Also add `mod update_check;` to `crates/app/src/main.rs` at this point (after `mod viewer;` on line 15):

```rust
mod update_check;
```

**Step 2.2** — [agent] Run the tests. Expected: **FAIL** because `compare_versions` is `todo!()`:

```bash
cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo test -p pairwire update_check -- --nocapture 2>&1
```

Expected output contains lines like:

```
test update_check::tests::older_current_returns_less ... FAILED
...
test result: FAILED. 0 passed; N failed
```

Or the process panics with `not yet implemented: implement in Step 2.3`. Either outcome confirms the RED state. Do NOT proceed to Step 2.3 until you observe at least one FAILED test.

**Step 2.3** — [agent] Replace the `todo!()` stubs with the real implementation. Edit `crates/app/src/update_check.rs`: replace the `compare_versions` body and the `spawn_update_check` body with the full implementations below.

Replace `compare_versions`:

```rust
pub fn compare_versions(current: &str, remote: &str) -> Option<Ordering> {
    fn parse(s: &str) -> Option<(u64, u64, u64)> {
        let mut parts = s.trim().splitn(4, '.');
        let major = parts.next()?.parse::<u64>().ok()?;
        let minor = parts.next()?.parse::<u64>().ok()?;
        let patch = parts.next()?.parse::<u64>().ok()?;
        // Reject if there are more than 3 dot-separated components
        if parts.next().is_some() {
            return None;
        }
        Some((major, minor, patch))
    }
    let (cm, cn, cp) = parse(current)?;
    let (rm, rn, rp) = parse(remote)?;
    Some((cm, cn, cp).cmp(&(rm, rn, rp)))
}
```

Replace `spawn_update_check`:

```rust
pub fn spawn_update_check(current_version: &'static str) -> watch::Receiver<Option<UpdateAvailable>> {
    const VERSION_JSON_URL: &str = "https://pairwire.dev/version.json";

    let (tx, rx) = watch::channel(None::<UpdateAvailable>);

    tokio::spawn(async move {
        debug!(current = current_version, url = VERSION_JSON_URL, "update check starting");

        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                debug!(error = %e, "update check: failed to build HTTP client");
                return;
            }
        };

        let response = match client.get(VERSION_JSON_URL).send().await {
            Ok(r) => r,
            Err(e) => {
                debug!(error = %e, "update check: network request failed");
                return;
            }
        };

        let info: VersionInfo = match response.json::<VersionInfo>().await {
            Ok(v) => v,
            Err(e) => {
                debug!(error = %e, "update check: failed to parse version.json");
                return;
            }
        };

        debug!(remote = %info.version, current = current_version, "update check: parsed version.json");

        match compare_versions(current_version, &info.version) {
            Some(Ordering::Less) => {
                tracing::info!(
                    remote = %info.version,
                    url = %info.url,
                    "update available"
                );
                // Ignore send error — the UI may have already exited.
                let _ = tx.send(Some(UpdateAvailable {
                    version: info.version,
                    url: info.url,
                    notes: info.notes,
                }));
            }
            Some(Ordering::Equal) => {
                debug!("update check: already on latest version");
            }
            Some(Ordering::Greater) => {
                debug!("update check: running ahead of published version (dev build)");
            }
            None => {
                debug!(
                    remote = %info.version,
                    current = current_version,
                    "update check: could not compare versions (malformed)"
                );
            }
        }
    });

    rx
}
```

**Step 2.4** — [agent] Run the tests. Expected: **PASS** (GREEN):

```bash
cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo test -p pairwire update_check -- --nocapture 2>&1
```

Expected output:

```
test update_check::tests::older_current_returns_less ... ok
test update_check::tests::equal_versions_returns_equal ... ok
test update_check::tests::newer_current_returns_greater ... ok
test update_check::tests::major_version_bump_detected ... ok
test update_check::tests::patch_version_bump_detected ... ok
test update_check::tests::malformed_current_returns_none ... ok
test update_check::tests::malformed_remote_returns_none ... ok
test update_check::tests::both_malformed_returns_none ... ok
test update_check::tests::empty_string_returns_none ... ok
test update_check::tests::too_many_components_returns_none ... ok
test update_check::tests::two_components_returns_none ... ok
test update_check::tests::leading_whitespace_is_trimmed ... ok
test update_check::tests::version_info_deserializes_valid_json ... ok
test update_check::tests::version_info_fails_on_missing_field ... ok
test update_check::tests::version_info_fails_on_empty_input ... ok
test update_check::tests::version_info_fails_on_wrong_type ... ok

test result: ok. 16 passed; 0 failed
```

Do NOT proceed until all 16 pass.

**Step 2.5** — [agent] Run `just lint` to check clippy passes:

```bash
cd /Users/bilal/Documents/personal/Github/rust-sharing && just lint
```

Expected: `0 warnings, 0 errors` for the `pairwire` crate.

**Step 2.6** — [agent] **Commit**:

```
git commit -m "feat(P4.6.4): add update_check module — compare_versions + VersionInfo + spawn_update_check"
```

---

### Task 3: Fix pre-existing `println!` violations in `session.rs`

**Files:**
- Modify `crates/app/src/session.rs` — lines 136–137

The project rule is "Never `println!` in library crates. Use `tracing`." `crates/app/src/session.rs` contains two `println!` calls at lines 136–137. `just lint` with `-D warnings` may flag these now that new code touches the same crate. Fix them before wiring `update_rx` to avoid a spurious `just all` failure in Task 7 unrelated to the new code.

- [ ] [agent] **Step 3.1** — In `crates/app/src/session.rs`, replace:

```rust
    println!("\n  Invite link:\n\n    {invite}\n");
    println!("  Waiting for guest to connect…\n");
```

with:

```rust
    tracing::info!(link = %invite, "invite link ready — share this with your guest");
    tracing::info!("waiting for guest to connect");
```

- [ ] [agent] **Step 3.2** — Verify:

```bash
cd /Users/bilal/Documents/personal/Github/rust-sharing && just lint
```

Expected: no `clippy::print_stdout` or similar warning for `session.rs`.

- [ ] [agent] **Step 3.3 — Commit**:

```
git commit -m "fix(P4.6.4): replace println! with tracing in session.rs (rule: no println in lib crates)"
```

---

### Task 4: Wire `spawn_update_check` into `main.rs` startup

**Files:**
- Modify `crates/app/src/main.rs` — add `spawn_update_check` call at the start of both command handlers; thread `update_rx` through to viewer and overlay.

The update check must fire **before** any blocking call (`block_on`, `tray poll loop`) so it doesn't delay UI appearance. The `watch::Receiver` is cheaply cloneable — one receiver can be shared with both the overlay and the viewer.

- [ ] [agent] **Step 4.1** — In `crates/app/src/main.rs`, modify `fn main()` to call `spawn_update_check` immediately after the tokio runtime is built, before dispatching on `cmd`.

Current code (lines 34–47):

```rust
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    match cmd {
        cli::Command::Host(args) => {
            run_host_command(args, &rt)?;
        }
        cli::Command::Join(args) => {
            run_join_command(args, &rt)?;
        }
    }
```

Replace with:

```rust
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // Fire the update check immediately inside the runtime. `spawn_update_check`
    // returns a watch::Receiver — the background HTTP request runs concurrently
    // while the rest of startup proceeds (binding iroh, launching the overlay).
    // `block_on` here does NOT block until the HTTP request completes — it only
    // runs the synchronous portion of `spawn_update_check` (create channels,
    // call `tokio::spawn`) and returns immediately.
    let update_rx = rt.block_on(async {
        update_check::spawn_update_check(env!("CARGO_PKG_VERSION"))
    });

    match cmd {
        cli::Command::Host(args) => {
            run_host_command(args, &rt, update_rx)?;
        }
        cli::Command::Join(args) => {
            run_join_command(args, &rt, update_rx)?;
        }
    }
```

- [ ] [agent] **Step 4.1a** — Add `update_rx` parameter to BOTH `run_host_with_overlay` and `run_host_with_overlay_and_tray` function signatures. These two functions are gated by `#[cfg(...)]` — update each signature to accept `update_rx` as a new last parameter:

**`run_host_with_overlay`** (cfg `#[cfg(all(feature = "gui", not(feature = "tray")))]`):
```rust
// Before:
fn run_host_with_overlay(args: cli::HostArgs, rt: &tokio::runtime::Runtime) -> Result<()>

// After:
fn run_host_with_overlay(
    args: cli::HostArgs,
    rt: &tokio::runtime::Runtime,
    update_rx: tokio::sync::watch::Receiver<Option<update_check::UpdateAvailable>>,
) -> Result<()>
```

**`run_host_with_overlay_and_tray`** (cfg `#[cfg(all(feature = "gui", feature = "tray"))]`):
```rust
// Before:
fn run_host_with_overlay_and_tray(args: cli::HostArgs, rt: &tokio::runtime::Runtime) -> Result<()>

// After:
fn run_host_with_overlay_and_tray(
    args: cli::HostArgs,
    rt: &tokio::runtime::Runtime,
    update_rx: tokio::sync::watch::Receiver<Option<update_check::UpdateAvailable>>,
) -> Result<()>
```

Thread `update_rx` through to the notification surface inside each function:
- In `run_host_with_overlay`: pass `update_rx` into `session::run_host(...)` (see Step 5.3) and into `TransparentHostOverlayApp::new` (see Task 5).
- In `run_host_with_overlay_and_tray`: same — pass `update_rx` into `session::run_host(...)` and into `TransparentHostOverlayApp::new`.

Without these signature changes, the call sites in `run_host_command` (Step 4.2) that pass `update_rx` will produce a **mismatched-argument-count error** from `just check`. Fix the signatures BEFORE updating the call sites.

- [ ] [agent] **Step 4.2** — Update `run_host_command` signature to accept `update_rx` and thread it to all inner call sites. The function currently dispatches to four inner functions. Each inner function needs `update_rx` passed through. The body dispatch also needs updating — Step 4.2 shows the full replacement.

Current (line 50):
```rust
fn run_host_command(args: cli::HostArgs, rt: &tokio::runtime::Runtime) -> Result<()> {
    #[cfg(feature = "gui")]
    {
        if args.no_overlay {
            run_host_no_overlay(args, rt)
        } else {
            #[cfg(feature = "tray")]
            {
                run_host_with_overlay_and_tray(args, rt)
            }
            #[cfg(not(feature = "tray"))]
            {
                run_host_with_overlay(args, rt)
            }
        }
    }

    #[cfg(all(feature = "tray", not(feature = "gui")))]
    {
        run_host_with_tray(args, rt)
    }

    #[cfg(not(any(feature = "gui", feature = "tray")))]
    {
        run_host_no_overlay(args, rt)
    }
}
```

Replace with (the body dispatch is updated to pass `update_rx`):

```rust
fn run_host_command(
    args: cli::HostArgs,
    rt: &tokio::runtime::Runtime,
    update_rx: tokio::sync::watch::Receiver<Option<update_check::UpdateAvailable>>,
) -> Result<()> {
    #[cfg(feature = "gui")]
    {
        if args.no_overlay {
            run_host_no_overlay(args, rt, update_rx)
        } else {
            #[cfg(feature = "tray")]
            {
                run_host_with_overlay_and_tray(args, rt, update_rx)
            }
            #[cfg(not(feature = "tray"))]
            {
                run_host_with_overlay(args, rt, update_rx)
            }
        }
    }

    #[cfg(all(feature = "tray", not(feature = "gui")))]
    {
        run_host_with_tray(args, rt, update_rx)
    }

    #[cfg(not(any(feature = "gui", feature = "tray")))]
    {
        run_host_no_overlay(args, rt, update_rx)
    }
}
```

- [ ] [agent] **Step 4.3** — Update `run_host_no_overlay` (line 88) to accept and drop `_update_rx`. The cfg guard `#[cfg(any(feature = "gui", not(feature = "tray")))]` is correct and must not change.

Current:
```rust
#[cfg(any(feature = "gui", not(feature = "tray")))]
fn run_host_no_overlay(args: cli::HostArgs, rt: &tokio::runtime::Runtime) -> Result<()> {
    rt.block_on(session::run_host(args, None, None, None, None))
}
```

Replace with:
```rust
#[cfg(any(feature = "gui", not(feature = "tray")))]
fn run_host_no_overlay(
    args: cli::HostArgs,
    rt: &tokio::runtime::Runtime,
    _update_rx: tokio::sync::watch::Receiver<Option<update_check::UpdateAvailable>>,
) -> Result<()> {
    rt.block_on(session::run_host(args, None, None, None, None))
}
```

- [ ] [agent] **Step 4.4** — Update `run_host_with_tray` (line 193, cfg `#[cfg(all(feature = "tray", not(feature = "gui")))]`) to accept and drop `_update_rx`. The tray-only path has no egui canvas to paint the banner on. The update notification is silently dropped in v1.

Add `_update_rx: tokio::sync::watch::Receiver<Option<update_check::UpdateAvailable>>` as the last parameter of `run_host_with_tray`. Do not use it.

- [ ] [agent] **Step 4.5** — Update `run_join_command` (line 285) to accept `update_rx` and thread it to `viewer::run_viewer`. Update signature:

```rust
fn run_join_command(
    args: cli::JoinArgs,
    rt: &tokio::runtime::Runtime,
    update_rx: tokio::sync::watch::Receiver<Option<update_check::UpdateAvailable>>,
) -> Result<()> {
```

For non-gui builds, drop it by adding `update_rx` to the existing `#[cfg(not(feature = "gui"))] let _ = (...)` binding. For gui builds, pass `update_rx` into `viewer::run_viewer(...)` (see Task 6 for viewer changes).

- [ ] [agent] **Step 4.6** — Verify compilation:

```bash
cd /Users/bilal/Documents/personal/Github/rust-sharing && just check
```

Expected: no errors. Unused-variable warnings about `_update_rx` in non-gui `#[cfg]` paths are silenced by the leading `_`.

- [ ] [agent] **Step 4.7 — Commit**:

```
git commit -m "feat(P4.6.4): wire spawn_update_check into main.rs startup, thread update_rx to UI surfaces"
```

---

### Task 5: Render the update banner in the transparent host overlay

**Files:**
- Modify `crates/app/src/session.rs` — add `update_rx` to `HostOverlayChannels`; add parameter to `run_host`
- Modify `crates/app/src/host_overlay_transparent.rs` — add `update_rx` field to `TransparentHostOverlayApp`; render banner in `update()`
- Modify `crates/app/src/main.rs` — thread `update_rx` into `run_host_with_overlay` and `run_host_with_overlay_and_tray`

The banner follows the exact same pill-rendering pattern as `render_session_banner` (lines 609–647 in `host_overlay_transparent.rs`) but is positioned one pill-height lower to avoid overlap. It is dismissed by clicking (using egui's `interact` on the pill's `Rect`).

**Passing `update_rx` through `run_host`:** `run_host` is an async function in `session.rs`. The cleanest approach is to pass `update_rx` directly as a parameter to `run_host` and have it include it in `HostOverlayChannels`. Because non-gui builds have no `HostOverlayChannels`, gate the parameter with `#[cfg(feature = "gui")]` / `#[cfg(not(feature = "gui"))]` matching the existing `overlay_tx` pattern.

- [ ] [agent] **Step 5.1** — In `crates/app/src/session.rs`, add `update_rx` to `HostOverlayChannels` (line 72):

```rust
pub struct HostOverlayChannels {
    // ... existing fields ...
    /// Update-check receiver — `Some` when a newer version is found on
    /// `https://pairwire.dev/version.json`.
    pub update_rx: tokio::sync::watch::Receiver<Option<crate::update_check::UpdateAvailable>>,
}
```

- [ ] [agent] **Step 5.2** — Add `update_rx` parameter to `run_host` (line 117). Follow the exact same `#[cfg]` gating pattern already used for `overlay_tx`:

Current signature:
```rust
pub async fn run_host(
    args: HostArgs,
    invite_tx: Option<tokio::sync::oneshot::Sender<String>>,
    tray_action_rx: Option<tokio::sync::mpsc::Receiver<crate::control_state::ControlAction>>,
    #[cfg(feature = "gui")] overlay_tx: Option<tokio::sync::oneshot::Sender<HostOverlayChannels>>,
    #[cfg(not(feature = "gui"))] _overlay_tx: Option<()>,
    mut shutdown_rx: Option<watch::Receiver<bool>>,
) -> Result<()> {
```

New signature (append `update_rx` after `shutdown_rx` using the same cfg-gating pattern):
```rust
pub async fn run_host(
    args: HostArgs,
    invite_tx: Option<tokio::sync::oneshot::Sender<String>>,
    tray_action_rx: Option<tokio::sync::mpsc::Receiver<crate::control_state::ControlAction>>,
    #[cfg(feature = "gui")] overlay_tx: Option<tokio::sync::oneshot::Sender<HostOverlayChannels>>,
    #[cfg(not(feature = "gui"))] _overlay_tx: Option<()>,
    mut shutdown_rx: Option<watch::Receiver<bool>>,
    #[cfg(feature = "gui")] update_rx: tokio::sync::watch::Receiver<Option<crate::update_check::UpdateAvailable>>,
    #[cfg(not(feature = "gui"))] _update_rx: tokio::sync::watch::Receiver<Option<crate::update_check::UpdateAvailable>>,
) -> Result<()> {
```

In the `HostOverlayChannels` construction site (around line 253), add `update_rx` to the struct literal:

```rust
let channels = HostOverlayChannels {
    // ... existing fields ...
    update_rx,
};
```

- [ ] [agent] **Step 5.3** — Update all `session::run_host(...)` call sites in `main.rs` to pass `update_rx` as the new last argument. There are four call sites:

**`run_host_no_overlay`** (existing call: `rt.block_on(session::run_host(args, None, None, None, None))`):
```rust
rt.block_on(session::run_host(args, None, None, None, None, _update_rx))
```

**`run_host_with_overlay`** (around line 100 — `rt.spawn(session::run_host(...))`). The `update_rx` is passed from the function parameter:
```rust
let session_handle = rt.spawn(session::run_host(
    args,
    None,
    None,
    Some(overlay_tx),
    Some(host_shutdown_rx),
    update_rx,
));
```

**`run_host_with_overlay_and_tray`** (around line 147):
```rust
let session_handle = rt.spawn(session::run_host(
    args,
    Some(invite_tx),
    Some(control_rx),
    Some(overlay_tx),
    Some(host_shutdown_rx),
    update_rx,
));
```

**`run_host_with_tray`** (around line 203 — tray-only, drop it):
```rust
let session_handle = rt.spawn(session::run_host(
    args,
    Some(invite_tx),
    Some(tray_action_rx),
    None,
    None,
    _update_rx,
));
```

- [ ] [agent] **Step 5.4** — In `host_overlay_transparent.rs`, add `update_rx` and `update_dismissed` fields to `TransparentHostOverlayApp` struct (around line 324). Add after `state_rx`:

```rust
        /// Update-check result from `spawn_update_check`. `Some` when a newer
        /// version is available; `None` until checked or if offline.
        update_rx: tokio::sync::watch::Receiver<Option<crate::update_check::UpdateAvailable>>,
        /// When `true`, the user has dismissed the update banner for this session.
        update_dismissed: bool,
```

- [ ] [agent] **Step 5.5** — Add `update_rx` to `TransparentHostOverlayApp::new` (line 374).

Current:
```rust
pub fn new(channels: HostOverlayChannels, capture_width: u32, capture_height: u32) -> Self {
```

New:
```rust
pub fn new(
    channels: HostOverlayChannels,
    capture_width: u32,
    capture_height: u32,
    update_rx: tokio::sync::watch::Receiver<Option<crate::update_check::UpdateAvailable>>,
) -> Self {
```

Add to the `Self { ... }` initializer (after `chrome_init: false`):
```rust
                update_rx,
                update_dismissed: false,
```

- [ ] [agent] **Step 5.6** — Add `render_update_banner` method to `impl TransparentHostOverlayApp`. Add this method after `render_session_banner`:

```rust
        /// Render the update-available notification banner.
        ///
        /// Positioned below the session-state banner. A semi-transparent pill with
        /// a clickable "Download" affordance. Clicking anywhere on the pill opens
        /// the download URL in the system browser and dismisses the banner for
        /// this session.
        ///
        /// Returns `true` if the user clicked the banner (caller should set
        /// `update_dismissed = true`).
        fn render_update_banner(
            &self,
            ui: &egui::Ui,
            canvas: egui::Rect,
            update: &crate::update_check::UpdateAvailable,
        ) -> bool {
            let label = format!(
                "Update available: v{} — click to download",
                update.version
            );
            let p = ui.painter();
            let font = egui::FontId::proportional(12.0);
            let text_color = egui::Color32::from_rgb(255, 220, 80);
            let galley = p.layout_no_wrap(label.clone(), font.clone(), text_color);
            let pad = egui::Vec2::new(10.0, 4.0);
            let label_size = galley.size() + pad * 2.0;
            // Position: top-center, one banner-height below the session banner.
            let label_x = canvas.center().x - label_size.x / 2.0;
            let label_y = canvas.min.y + 6.0 + 22.0; // 22px below the session banner
            let label_pos = egui::pos2(label_x, label_y);
            let bg_rect = egui::Rect::from_min_size(label_pos, label_size);
            p.rect_filled(
                bg_rect,
                6.0,
                egui::Color32::from_rgba_unmultiplied(10, 10, 10, 160),
            );
            p.text(
                label_pos + pad,
                egui::Align2::LEFT_TOP,
                label,
                font,
                text_color,
            );

            // Hit-test: return true if the user clicked inside the pill.
            // `interact` registers the rect with egui's input system without
            // adding a visible widget — consistent with the painter-only approach
            // used throughout this overlay.
            let response = ui.interact(
                bg_rect,
                ui.id().with("update_banner"),
                egui::Sense::click(),
            );
            if response.clicked() {
                // Open the download URL in the system browser.
                // `open::that` is not in the workspace — use the platform shell
                // command directly to avoid a new dependency.
                #[cfg(target_os = "macos")]
                {
                    let _ = std::process::Command::new("open").arg(&update.url).spawn();
                }
                #[cfg(target_os = "linux")]
                {
                    let _ = std::process::Command::new("xdg-open").arg(&update.url).spawn();
                }
                #[cfg(target_os = "windows")]
                {
                    let _ = std::process::Command::new("cmd")
                        .args(["/C", "start", &update.url])
                        .spawn();
                }
                return true;
            }
            false
        }
```

- [ ] [agent] **Step 5.7** — Call `render_update_banner` from `update()`. In the `impl eframe::App for TransparentHostOverlayApp` `update` method, inside the `egui::CentralPanel::default()...show(ctx, |ui| { ... })` block, after `self.render_session_banner(ui, canvas);`:

```rust
                    // Render update notification if available and not dismissed.
                    // Note: borrow() is dropped before the mutation of
                    // self.update_dismissed to avoid a borrow conflict.
                    if !self.update_dismissed {
                        let update = self.update_rx.borrow().clone();
                        if let Some(update) = update {
                            if self.render_update_banner(ui, canvas, &update) {
                                // User clicked — dismiss for this session.
                                self.update_dismissed = true;
                            }
                        }
                    }
```

- [ ] [agent] **Step 5.8** — Update `TransparentHostOverlayApp::new` call sites in `run_transparent_host_overlay` and `run_transparent_host_overlay_with_tray`. Both construct `TransparentHostOverlayApp`. Extract `update_rx` from `channels` before passing.

For `run_transparent_host_overlay`:

Before:
```rust
Ok(Box::new(inner::TransparentHostOverlayApp::new(channels, cw, ch)))
```

After:
```rust
let update_rx = channels.update_rx.clone();
Ok(Box::new(inner::TransparentHostOverlayApp::new(channels, cw, ch, update_rx)))
```

For `run_transparent_host_overlay_with_tray` (the tray variant constructs `TransparentHostOverlayApp` directly — locate the actual construction site by searching for `TransparentHostOverlayApp::new` in the file, then apply the same pattern):

```rust
let update_rx = channels.update_rx.clone();
// ... (pass update_rx as the 4th argument to TransparentHostOverlayApp::new)
inner::TransparentHostOverlayApp::new(channels, capture_width, capture_height, update_rx)
```

- [ ] [agent] **Step 5.9** — Verify compilation:

```bash
cd /Users/bilal/Documents/personal/Github/rust-sharing && just check
```

Expected: clean.

- [ ] [agent] **Step 5.10 — Commit**:

```
git commit -m "feat(P4.6.4): render update-available banner in transparent host overlay"
```

---

### Task 6: Render the update banner in the guest viewer

**Files:**
- Modify `crates/app/src/viewer.rs` — add `update_rx` field to `ViewerApp`, render banner in `update()`

The viewer already has a HUD and connection-state banner. The update banner is added as a second pill using the same egui painter approach.

Note: `ViewerApp::new` already carries a `#[allow(clippy::too_many_arguments)]` attribute. Do NOT duplicate that attribute when adding the new parameter.

- [ ] [agent] **Step 6.1** — In `crates/app/src/viewer.rs`, locate the `struct ViewerApp` definition (around line 168) and add two fields after `control_tx`:

```rust
    /// Update-check receiver. `Some` when a newer version is found.
    update_rx: tokio::sync::watch::Receiver<Option<crate::update_check::UpdateAvailable>>,
    /// `true` once the user has clicked the update banner for this session.
    update_dismissed: bool,
```

- [ ] [agent] **Step 6.2** — Update `ViewerApp::new` signature (around line 190, after the struct definition at ~168):

Add `update_rx: tokio::sync::watch::Receiver<Option<crate::update_check::UpdateAvailable>>` as the last parameter (after `control_tx`), and store it in `Self { ..., update_rx, update_dismissed: false }`. The existing `#[allow(clippy::too_many_arguments)]` attribute already present on this function covers the new parameter — do not add another.

- [ ] [agent] **Step 6.3** — Add `render_update_banner` method to `impl ViewerApp`. The viewer is an opaque window (not a transparent overlay), so use `ui.add(egui::Label::new(...).sense(...))` rather than the raw painter path:

```rust
    fn render_update_banner(
        &mut self,
        ui: &mut egui::Ui,
    ) {
        if self.update_dismissed {
            return;
        }
        // Drop the borrow before any mutation to avoid borrow conflict.
        let update = {
            let borrow = self.update_rx.borrow();
            borrow.clone()
        };
        let Some(update) = update else { return };

        let label = format!(
            "Update available: v{} — click to download",
            update.version
        );
        let response = ui.add(
            egui::Label::new(
                egui::RichText::new(label)
                    .color(egui::Color32::from_rgb(255, 220, 80))
                    .size(12.0),
            )
            .sense(egui::Sense::click()),
        );
        if response.clicked() {
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open").arg(&update.url).spawn();
            }
            #[cfg(target_os = "linux")]
            {
                let _ = std::process::Command::new("xdg-open").arg(&update.url).spawn();
            }
            #[cfg(target_os = "windows")]
            {
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "start", &update.url])
                    .spawn();
            }
            self.update_dismissed = true;
        }
    }
```

- [ ] [agent] **Step 6.4** — In the viewer's `update()` method (inside `impl eframe::App for ViewerApp`), locate the `egui::CentralPanel::default().show(ctx, |ui| { ... })` block. Find where `render_session_banner` or the connection banner is rendered (search for `render_` or `conn_rx.borrow()` in the `update` method body). Add a call to `self.render_update_banner(ui)` immediately after that banner call.

Concrete anchor: find the line in `update()` that calls a banner render or the HUD render, and insert directly after it:

```rust
            self.render_update_banner(ui);
```

- [ ] [agent] **Step 6.5** — Update `run_viewer` signature to accept `update_rx`:

```rust
    pub fn run_viewer(
        frame_rx: watch::Receiver<Option<DecodedFrame>>,
        cursor_rx: watch::Receiver<Option<CursorUpdate>>,
        conn_rx: watch::Receiver<ConnectionState>,
        input_tx: broadcast::Sender<InputMessage>,
        drawing_rx: watch::Receiver<Vec<TimedStroke>>,
        live_point_rx: watch::Receiver<Option<DrawPoint>>,
        drawing_tx: broadcast::Sender<DrawingEvent>,
        mute_tx: watch::Sender<bool>,
        quality_rx: watch::Receiver<QualityTier>,
        control_rx: watch::Receiver<RuntimeControlState>,
        control_tx: broadcast::Sender<ControlMessage>,
        title: &str,
        update_rx: tokio::sync::watch::Receiver<Option<crate::update_check::UpdateAvailable>>,
    ) -> anyhow::Result<()> {
```

Pass `update_rx` into `ViewerApp::new(...)` inside the `Box::new(|_cc| { ... })` closure.

- [ ] [agent] **Step 6.6** — Update the `run_viewer(...)` call in `main.rs` `run_join_command` to pass `update_rx.clone()`.

- [ ] [agent] **Step 6.7** — Verify compilation + tests:

```bash
cd /Users/bilal/Documents/personal/Github/rust-sharing && just check
cargo test -p pairwire update_check -- --nocapture 2>&1
```

Expected: clean compile, 16 tests pass.

- [ ] [agent] **Step 6.8 — Commit**:

```
git commit -m "feat(P4.6.4): render update-available banner in guest viewer"
```

---

### Task 7: Full validation pass

**Files:** none new — read-only verification.

- [ ] [agent] **Step 7.1** — Run the full workspace check:

```bash
cd /Users/bilal/Documents/personal/Github/rust-sharing && just all
```

Expected: `cargo check`, `cargo clippy`, `cargo test`, `cargo fmt --check` all pass. Zero warnings in the `pairwire` crate (the `-D warnings` flag in `just lint` is enforced).

- [ ] [agent] **Step 7.2** — Verify `update_check` tests specifically:

```bash
cd /Users/bilal/Documents/personal/Github/rust-sharing && cargo test -p pairwire update_check 2>&1
```

Expected: `test result: ok. 16 passed; 0 failed; 0 ignored`.

- [ ] [agent] **Step 7.3** — Manual smoke test: network offline fails silently.

```bash
cd /Users/bilal/Documents/personal/Github/rust-sharing && RUST_LOG=pairwire=debug cargo run --features encode,capture,audio,gui,tray -p pairwire -- host --no-overlay 2>&1 | head -20
```

Expected: `DEBUG pairwire::update_check: update check starting` appears, then either `DEBUG pairwire::update_check: update check: network request failed` (before Workstream C is live) or a version comparison log. The binary must continue to full startup — no hang, no panic, no `ERROR`-level log from the update check module.

- [ ] [agent] **Step 7.4** — Manual smoke test: verify version check fires with a stale CARGO_PKG_VERSION.

Because `CARGO_PKG_VERSION` is baked at compile time (not a runtime env var), the correct procedure is to build with a patched version, then run:

```bash
cd /Users/bilal/Documents/personal/Github/rust-sharing
# Temporarily set version in crates/app/Cargo.toml to 0.0.1:
#   version = "0.0.1"
# Then build:
cargo build --features encode,capture,audio,gui,tray -p pairwire
# Then run and observe logs:
RUST_LOG=pairwire=debug ./target/debug/pairwire host --no-overlay 2>&1 | head -20
# Then restore the original version in crates/app/Cargo.toml.
```

Expected (once Workstream C's `/version.json` is live): `INFO pairwire::update_check: update available remote="X.Y.Z" url="https://pairwire.dev/download"`. Before C is live: `DEBUG ... network request failed` — both outcomes are correct.

- [ ] **[human] Step 7.5 — Prerequisite gate**: Before the smoke test in Step 7.4 can verify a successful update notification, Manual Task #1 (register `pairwire.dev`) and Manual Task #3 (deploy Cloudflare Pages with `/version.json`) from the spec must be complete. An agent executor will observe `DEBUG ... network request failed` until then — this is correct fail-silent behavior, NOT a bug. Do not block the commit on this.

- [ ] [agent] **Step 7.6 — Final commit**:

```
git commit -m "feat(P4.6.4): complete update check — all validation steps pass"
```

---

## Dependency decision rationale

### HTTP client: `reqwest` (not `ureq`)

`reqwest` 0.12.28 is **already in `Cargo.lock`** as a transitive dependency of iroh 0.97 (confirmed at lines 2796 and 2892 of `Cargo.lock`). Adding it as an explicit `[dependencies]` entry in `crates/app/Cargo.toml` introduces zero new crates and zero additional compile time — cargo pins to the already-resolved 0.12.28. `ureq` would be a genuinely new dependency (smaller binary, sync API) but is unjustified here because: (a) we are already in an async context, (b) reqwest is already compiled, (c) adding ureq would add a new crate to the lock file.

The minimal feature set used: `json` (for `.json::<VersionInfo>()`) and `rustls-tls` (already in the iroh dependency tree; avoids a native TLS dependency). No `blocking` feature is needed — the async path is used.

### Semver: hand-rolled, not the `semver` crate

`semver` 1.0.27 is in `Cargo.lock` transitively (confirmed). However, the comparison is `major.minor.patch` with no pre-release/build-metadata semantics needed for a simple "is this newer?" check. The 30-line hand-rolled `compare_versions` function is simpler to audit and has 0 indirect compile-time cost. If pre-release version handling becomes necessary, the `semver` crate can be added explicitly at that point.

### JSON: `serde_json` (not `serde` alone)

`serde_json` 1.0.149 is already in `Cargo.lock` via `reqwest`. Explicit dependency needed for the `serde_json::from_str` call in tests and the `.json::<VersionInfo>()` deserialization (which uses `serde_json` under the hood via `reqwest`'s `json` feature).

---

## Dependency on Workstream C

The `VERSION_JSON_URL` constant (`https://pairwire.dev/version.json`) is hard-coded. The file shape is:

```json
{ "version": "0.2.0", "url": "https://pairwire.dev/download", "notes": "Bug fixes" }
```

**All three fields are required.** Missing or wrong-type fields cause a `serde_json` parse error, which is caught and logged at `debug!` level — the app continues normally. This means the update check degrades gracefully even if Workstream C deploys a different shape temporarily.

**URL validation note:** the `url` field from `version.json` is passed directly to `std::process::Command::new("open")`. If Workstream C ever ships a non-URL value (bare path, empty string), the `open` command will silently fail with no log entry. Callers should be aware: the current implementation does not validate `url` format before invoking the browser. This is acceptable for v1 given we fully control `version.json`; add a `url::Url::parse` guard if the field is ever sourced from user input.

Before Workstream C's `/version.json` endpoint is live (domain not registered yet — see Manual tasks #1, #3 in the spec), every run will log `DEBUG ... network request failed`. This is correct and expected — see Step 7.3 above.

**[human] dependency:** `pairwire.dev` domain registration (spec Manual Task #1) and Cloudflare Pages deployment (spec Manual Task #3) must complete before the update check can resolve. No code changes are needed once those are live.

---

## Notification surface coverage

| Build config | Surface | Coverage |
|---|---|---|
| `gui + tray` (default release) | Transparent overlay banner pill (host view) | Full — Task 5 |
| `gui` only | Transparent overlay banner pill (host view) | Full — Task 5 |
| `gui` guest viewer | Viewer HUD banner label | Full — Task 6 |
| `tray` only, no gui | Silently dropped (no canvas) | v1 limitation — logged at `info!` level only (spec deviation: Workstream E says "wire into existing tray/overlay surface"; tray-only has no egui canvas; tracked as deferred fast-follow — add a tray menu item "Update available (vX.Y.Z)" post-v1) |
| Headless (`--no-overlay`, no gui) | Logged at `info!` level only | Expected — no UI surface |

The tray-only path logs `INFO pairwire::update_check: update available` but does not show a visual banner. A future fast-follow can add a tray menu item "Update available (vX.Y.Z)" using `TrayHandle::set_update_available` — not in scope for P4.6.4.
