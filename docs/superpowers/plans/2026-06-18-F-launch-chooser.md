# Launch Host/Join Chooser Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a double-clicked `pairwire.app` (no CLI args) present a native "Host or Join?" chooser and dispatch to the existing host/join flows, so the DMG is usable without a terminal.

**Architecture:** No-arg launch resolves to a new `Command::Launcher`. `main()` dispatches it to `run_launcher()`, which shows a **native macOS `NSAlert`** chooser (via `objc2-app-kit`, already a dependency) and returns a `LaunchChoice`. It then calls the existing `run_host_command` / `run_join_command` **in the same process**. The chooser MUST be native, not an `eframe` window, because winit permits only one `EventLoop` per process — an egui launcher would consume it and the subsequent overlay/viewer `eframe::run_native` would panic ("EventLoop can't be recreated"). The native dialog creates no EventLoop, so the host/join eframe loop starts cleanly afterward.

**Tech Stack:** Rust, `objc2-app-kit` 0.3 (`NSAlert`, `NSTextField`), existing `cli`/`session` modules, egui/eframe (unchanged, downstream of the choice).

**PLAN.md task ID(s):** P4.5.4 (Opinionated GUI Mode — launch chooser slice)

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/app/src/cli.rs` | **Modify** — add `Command::Launcher`; no-arg → `Ok(Command::Launcher)` instead of `bail!`; keep `--version`/`--help` precedence. Add a pure `validate_invite(&str) -> Result<()>`. |
| `crates/app/src/launcher.rs` | **Create** — `LaunchChoice` enum + `show_launcher() -> Result<Option<LaunchChoice>>` (native NSAlert chooser). macOS-gated; non-macOS fallback documented. |
| `crates/app/src/main.rs` | **Modify** — `mod launcher;`; dispatch `Command::Launcher => run_launcher(&rt)?`; add `run_launcher` that calls `show_launcher()` then `run_host_command`/`run_join_command`. |

## Constraints (read before coding)

- **One EventLoop per process (winit).** Do NOT build the chooser with eframe/egui. Use `NSAlert`. The host overlay (`run_host_with_overlay_and_tray`) and the viewer (`run_viewer`) each create the process's single eframe EventLoop *after* the choice is made.
- **Verify `objc2-app-kit` 0.3 API against the installed crate** (`~/.cargo/registry/.../objc2-app-kit-0.3*`) before writing — the existing `tray.rs` and `host_overlay_transparent.rs` already use `NSApplication`, `MainThreadMarker`, `ns_string!`; follow their patterns. `NSAlert`/`NSTextField` method names (`addButtonWithTitle:`, `runModal`, `setAccessoryView:`) must be confirmed.
- Must run on the **main thread** (AppKit requirement). `main()` is the main thread; call the chooser there before spawning the eframe loop.
- An accessory app (`LSUIElement=true`) can still show an `NSAlert`. `NSApplication::sharedApplication` must exist first; create it (or `setActivationPolicy(Regular)` for the dialog) so the alert comes to the foreground, then the host/overlay path manages activation as today.
- No `unwrap()` outside tests; no `println!` in lib code (this is the app binary — `tracing` is already initialised in `main`).

---

## Tasks

### Task 1: `Command::Launcher` + invite validation in `cli.rs`

**Files:**
- Modify: `crates/app/src/cli.rs:9-16` (enum), `crates/app/src/cli.rs:101-144` (`parse_from_args`), tests at `crates/app/src/cli.rs:279-288`
- Test: same file `#[cfg(test)] mod tests`

- [ ] **Step 1: Write the failing test** [agent]

Add to `mod tests`:
```rust
    #[test]
    fn parse_no_args_returns_launcher() {
        let cmd = parse(&[]).unwrap();
        assert!(matches!(cmd, Command::Launcher));
    }

    #[test]
    fn validate_invite_accepts_pairwire_url() {
        assert!(validate_invite("pairwire://abc?token=0000000000000000000000000000000000").is_ok());
    }

    #[test]
    fn validate_invite_rejects_empty_and_non_pairwire() {
        assert!(validate_invite("").is_err());
        assert!(validate_invite("https://example.com").is_err());
    }
```
Also DELETE the now-wrong `parse_no_args_errors` test (no-args is no longer an error).

- [ ] **Step 2: Run test to verify it fails** [agent]

Run: `cargo test -p pairwire --lib cli::tests::parse_no_args_returns_launcher`
Expected: FAIL — `Command::Launcher` does not exist / `parse(&[])` still returns `Err`.

- [ ] **Step 3: Implement** [agent]

Add the variant:
```rust
#[derive(Debug)]
pub enum Command {
    /// Start a hosting session and print an invite link.
    Host(HostArgs),
    /// Join a session using an invite link.
    Join(JoinArgs),
    /// No subcommand given (double-clicked app): show the native Host/Join chooser.
    Launcher,
}
```
Change the `None` arm of `parse_from_args` from `bail!` to:
```rust
        None => Ok(Command::Launcher),
```
Add the pure validator (used by both the chooser and `join` parsing):
```rust
/// Validate an invite URL at the boundary. Pairwire invites are `pairwire://...`.
pub fn validate_invite(url: &str) -> anyhow::Result<()> {
    let url = url.trim();
    if url.is_empty() {
        anyhow::bail!("invite link is empty");
    }
    if !url.starts_with("pairwire://") {
        anyhow::bail!("invite link must start with pairwire://");
    }
    Ok(())
}
```
(Optionally call `validate_invite` inside the `Some("join")` arm before constructing `JoinArgs` so the CLI path validates too.)

- [ ] **Step 4: Run tests to verify they pass** [agent]

Run: `cargo test -p pairwire --lib cli::`
Expected: PASS — all cli tests green (the deleted `parse_no_args_errors` is gone).

- [ ] **Step 5: Commit** [agent]
```bash
cd <worktree>
git add crates/app/src/cli.rs
git commit -m "feat(P4.5.4): add Command::Launcher for no-arg launch + invite validation"
```

### Task 2: `launcher.rs` — native NSAlert chooser

**Files:**
- Create: `crates/app/src/launcher.rs`
- Modify: `crates/app/src/main.rs:7-15` (add `mod launcher;`)

- [ ] **Step 1: Confirm the objc2-app-kit NSAlert API** [agent]

Run (inspect the installed crate, per the project rule that objc2/iroh APIs differ from docs):
```bash
ls ~/.cargo/registry/src/*/objc2-app-kit-0.3*/src/generated/NSAlert.rs 2>/dev/null && \
  grep -nE 'addButtonWithTitle|runModal|setAccessoryView|setMessageText|setInformativeText|fn alert' ~/.cargo/registry/src/*/objc2-app-kit-0.3*/src/generated/NSAlert.rs | head
grep -nE 'fn initWithFrame|fn stringValue|fn setStringValue' ~/.cargo/registry/src/*/objc2-app-kit-0.3*/src/generated/NSTextField.rs | head
```
Expected: method names exist (e.g. `addButtonWithTitle`, `runModal`, `setAccessoryView`). Note the exact signatures and required `features = [...]` for `objc2-app-kit` (add `NSAlert`, `NSTextField`, `NSView` to the macOS dependency feature list in `crates/app/Cargo.toml:55` if missing).

- [ ] **Step 2: Implement the chooser** [agent]

`crates/app/src/launcher.rs` (macOS impl; non-macOS fallback below). Follow the `MainThreadMarker` / `ns_string!` patterns already in `tray.rs`:
```rust
//! Native launch chooser shown when pairwire is started with no subcommand
//! (e.g. double-clicked from /Applications). MUST be native (NSAlert), not
//! eframe — winit allows only one EventLoop per process and the host/viewer
//! consume it after the choice is made.

use crate::cli::{self, HostArgs, JoinArgs};

/// What the user chose in the launch dialog.
#[derive(Debug)]
pub enum LaunchChoice {
    Host(HostArgs),
    Join(JoinArgs),
}

#[cfg(target_os = "macos")]
pub fn show_launcher() -> anyhow::Result<Option<LaunchChoice>> {
    use objc2_app_kit::{NSAlert, NSApplication, NSApplicationActivationPolicy, NSTextField};
    use objc2_foundation::{ns_string, MainThreadMarker, NSRect, NSPoint, NSSize};

    let mtm = MainThreadMarker::new()
        .ok_or_else(|| anyhow::anyhow!("launcher must run on the main thread"))?;
    let app = NSApplication::sharedApplication(mtm);
    // Make the dialog frontmost even for an accessory (LSUIElement) bundle.
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    // SAFETY: AppKit object construction on the main thread; mtm proves the thread.
    let alert = unsafe { NSAlert::new(mtm) };
    unsafe {
        alert.setMessageText(ns_string!("Pairwire"));
        alert.setInformativeText(ns_string!(
            "Host a session to share your screen, or paste an invite link to join."
        ));
        // Buttons return codes 1000, 1001, 1002 in add order.
        alert.addButtonWithTitle(ns_string!("Host")); // 1000
        alert.addButtonWithTitle(ns_string!("Join")); // 1001
        alert.addButtonWithTitle(ns_string!("Cancel")); // 1002

        // Accessory text field for the invite link (used only when Join is chosen).
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(300.0, 24.0));
        let field = NSTextField::initWithFrame(NSTextField::alloc(mtm), frame);
        field.setPlaceholderString(Some(ns_string!("pairwire://… (only needed to Join)")));
        alert.setAccessoryView(Some(&field));

        let response = alert.runModal();
        match response.0 {
            1000 => Ok(Some(LaunchChoice::Host(HostArgs::default()))),
            1001 => {
                let invite = field.stringValue().to_string();
                match cli::validate_invite(&invite) {
                    Ok(()) => Ok(Some(LaunchChoice::Join(JoinArgs {
                        invite_url: invite.trim().to_string(),
                        no_gui: false,
                    }))),
                    Err(e) => {
                        // Re-show a simple error alert, then return None (user can relaunch).
                        let err = NSAlert::new(mtm);
                        err.setMessageText(ns_string!("Invalid invite link"));
                        err.setInformativeText(ns_string!("Paste a link that starts with pairwire://"));
                        err.addButtonWithTitle(ns_string!("OK"));
                        err.runModal();
                        tracing::warn!(error = %e, "launcher: invalid invite, exiting");
                        Ok(None)
                    }
                }
            }
            _ => Ok(None), // Cancel
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn show_launcher() -> anyhow::Result<Option<LaunchChoice>> {
    // Non-macOS is not a v1 target; a launched-with-no-args binary prints guidance.
    anyhow::bail!("no subcommand given — run `pairwire host` or `pairwire join <invite>`");
}
```
**Note:** the exact `NSAlert::new` / `addButtonWithTitle` / `setAccessoryView` / `initWithFrame` signatures and which are `unsafe` depend on `objc2-app-kit` 0.3 — adjust to the API confirmed in Step 1 (wrap object creation in `unsafe` with a `// SAFETY:` note as the existing `host_overlay_transparent.rs` does). `response.0` is the `NSModalResponse` newtype inner `isize`.

- [ ] **Step 3: Register the module** [agent]

In `crates/app/src/main.rs` add `mod launcher;` alongside the other `mod` lines (after `mod cli;`).

- [ ] **Step 4: Build check** [agent]

Run: `cargo check -p pairwire --features gui,tray`
Expected: compiles (0 errors). Fix any objc2 signature mismatches against the Step 1 findings.

- [ ] **Step 5: Commit** [agent]
```bash
git add crates/app/src/launcher.rs crates/app/src/main.rs crates/app/Cargo.toml
git commit -m "feat(P4.5.4): native NSAlert host/join launch chooser"
```

### Task 3: Dispatch `Command::Launcher` in `main.rs`

**Files:**
- Modify: `crates/app/src/main.rs:38-48` (match), add `run_launcher`

- [ ] **Step 1: Implement dispatch** [agent]

Extend the match in `main()`:
```rust
    match cmd {
        cli::Command::Host(args) => run_host_command(args, &rt)?,
        cli::Command::Join(args) => run_join_command(args, &rt)?,
        cli::Command::Launcher => run_launcher(&rt)?,
    }
```
Add the function (gui-gated; without gui there is no overlay/viewer, so the launcher is gui-only — under `not(gui)` the `Launcher` arm should error with guidance):
```rust
#[cfg(feature = "gui")]
fn run_launcher(rt: &tokio::runtime::Runtime) -> Result<()> {
    match launcher::show_launcher()? {
        Some(launcher::LaunchChoice::Host(args)) => run_host_command(args, rt),
        Some(launcher::LaunchChoice::Join(args)) => run_join_command(args, rt),
        None => {
            info!("launcher: cancelled");
            Ok(())
        }
    }
}

#[cfg(not(feature = "gui"))]
fn run_launcher(_rt: &tokio::runtime::Runtime) -> Result<()> {
    anyhow::bail!("no subcommand — build with --features gui for the launch chooser, or run `pairwire host`");
}
```

- [ ] **Step 2: Verify it builds (all relevant feature combos)** [agent]

Run:
```bash
cargo check -p pairwire --features gui,tray
cargo check -p pairwire --no-default-features --features tray
```
Expected: both compile. The second (no gui) exercises the `not(gui)` `run_launcher` arm.

- [ ] **Step 3: Manual launch verification** [agent — local run; do NOT require a clean VM]

Run the built binary with NO args from the worktree:
```bash
FFMPEG_DIR="$(brew --prefix ffmpeg)" cargo run -p pairwire --features gui,tray,encode,capture,audio
```
Expected: a native dialog titled "Pairwire" appears with **Host / Join / Cancel** buttons and an invite text field.
- Click **Host** → the dialog closes and the host overlay + tray start; an invite link is printed/copyable (same as `pairwire host`).
- Relaunch, paste a `pairwire://…` link, click **Join** → the viewer window opens (same as `pairwire join <url>`).
- Relaunch, click **Cancel** → process exits cleanly (code 0).
Record the observed result in the commit message / a LEARNINGS note if any objc2 quirk was hit.

- [ ] **Step 4: Commit** [agent]
```bash
git add crates/app/src/main.rs
git commit -m "feat(P4.5.4): dispatch no-arg launch to native chooser → host/join"
```

### Task 4: Packaging note — chooser is the default-launch action

**Files:**
- Modify: `docs/superpowers/plans/2026-06-18-A-macos-dmg-packaging.md` (one cross-reference line) — only if not already noted.

- [ ] **Step 1:** Add a line under Plan A's bundle task noting that the packaged binary's no-arg entry now shows the chooser (P4.5.4), so the `.app`'s default double-click action is the Host/Join dialog — no CLI needed. [agent]
- [ ] **Step 2: Commit** [agent]
```bash
git commit -am "docs(P4.5.4): note launch chooser as the .app default action"
```

## Verification (whole feature)

- `cargo test -p pairwire --lib cli::` green (Launcher parse + invite validation).
- `cargo check -p pairwire --features gui,tray` and `--no-default-features --features tray` both compile.
- Manual: no-arg run shows the native chooser; Host starts hosting; Join (valid link) opens the viewer; Cancel exits 0.
- After Plan A repackages, double-clicking `/Applications/pairwire.app` shows the chooser instead of doing nothing.

## Out of scope (later)

- Rich egui launcher window (would need the re-exec approach: spawn a fresh `pairwire host`/`join` child, because of the one-EventLoop-per-process limit). Defer.
- Host options (fps/resolution) in the chooser — v1 uses `HostArgs::default()`.
- Remembering the last choice / recent invites.
