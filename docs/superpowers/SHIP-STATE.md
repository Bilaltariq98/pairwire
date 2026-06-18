# Pairwire v1 Ship — State & Merge Plan (2026-06-18)

Durable handoff so work survives a `/compact`. Updated as we go.

## Installed app (working)
`/Applications/pairwire.app` is built from branch `feat/ship-A-macos-dmg` and WORKS:
double-click → native Host/Join chooser; 14 FFmpeg dylibs bundled + `disable-library-validation`
entitlement (no dyld crash); Join field paste + clipboard pre-fill; Screen Recording perms gate
(no blue screen); host overlay now paints host-drawn strokes (self-echo) + Cmd+Shift+D focus fix.
The `.dmg` (`target/release/pairwire_0.1.0_aarch64.dmg`) is self-contained (verified).

## Branches (all unpushed, off base `feat/tray-overlay-unify`)
- `feat/tray-overlay-unify` (main working): planning docs, specs, plans A–F, PRODUCT.md, DESIGN.md, SHIP-STATE.md, site/ (being built). Latest doc commits: relay-selection, launch-chooser/crash-fix, copy, brand.
- `feat/ship-A-macos-dmg` (worktree .worktrees/ship-A): DMG packaging, cli `--version`/`--help`, launcher (P4.5.4), permissions gate, host-overlay drawing fix, dylib bundling, justfile package/dmg/bundle-dylibs (hdiutil DMG), Info.plist/entitlements, release.yml.
- `feat/ship-B-relay` (worktree .worktrees/ship-B): core NetworkConfig + `resolve_relay_mode` (debug=n0-public/release=ours-only, env overrides), infra/terraform/ (hcloud+cloudflare), justfile relay recipes, cloud-init. NOTE: B commits mis-reference task IDs `P4.5.1`/`P5` (should be `P4.6.1`) — cosmetic.
- `feat/ship-D-telemetry` (worktree .worktrees/ship-D): crates/telemetry (config/consent/events/posthog/sentry, EU, opt-in gate, tests green via shared TELEMETRY_TEST_LOCK), app integration + first-run consent dialog in overlay, main.rs init.

## Merge plan (do after site build completes, to keep ROOT clean)
Target: merge into `feat/tray-overlay-unify` (or a fresh `feat/ship-v1`). Suggested order A → B → D
(A is the biggest; B/D layer on). Expect conflicts in files touched on multiple branches:
- `LEARNINGS.md` — appended on A, B, D (and base). Resolve = keep all entries.
- `crates/app/Cargo.toml` — A (objc2 features, reqwest, serde) + D (pairwire-telemetry dep).
- `crates/app/src/main.rs` — A (launcher dispatch, perms gate, `--version`) + D (telemetry init, consent dialog wiring). Both edit `run_host_command` / startup — merge carefully.
- `crates/app/src/host_overlay_transparent.rs` — A (drawing self-echo, focus fix) + D (first-run consent dialog click-through). Both edit the overlay app — merge carefully.
- `justfile` — A (package/dmg/bundle-dylibs/sign) + B (relay-plan/apply/destroy). Disjoint recipes; easy.
- root `Cargo.toml` workspace members — D adds `crates/telemetry`.
After merging: `just all` (fmt+check+lint+test). Build the packaged app once more (`cd .worktrees/ship-A`-equivalent or from merged tree) and re-validate launch.

## Background workflows (results in /private/tmp/.../tasks/<id>.output)
- Site build: `wf_d941806e-00a` (task w7r6edaba) — writes site/ (index.html, styles.css, download.js, version.json, assets/logo.png). Build → craft-review → fix.

## Remaining after merge
- **Site:** web-animation-design pass (motion polish on the hero connect-motif), then serve + visual check. Repo = github.com/Bilaltariq98/pairwire; DMG latest-download link uses stable name `pairwire-macos.dmg` (release.yml must publish that asset name).
- **Human/manual (unblock deploy):** register pairwire.dev; Hetzner + Cloudflare + PostHog(EU) + Sentry(EU) accounts/tokens; SSH key; TF state secret; license (BSL?); real app icon from logo.png (`iconutil` → assets/pairwire.icns, replace placeholder).
- **Deploy:** `terraform apply` (relay), Cloudflare Pages (site), wire telemetry keys, cut a GitHub Release with the dmg.
- **QoL fast-follow:** host-tray mute toggle, connection-quality popover, reconnecting banner, who-joined toast.
- **App icon:** logo.png (1024²) → real `.icns` (currently checkerboard placeholder).
