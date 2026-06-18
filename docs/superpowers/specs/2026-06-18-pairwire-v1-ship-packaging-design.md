# Pairwire v1 Ship — Packaging & Distribution Design

**Date:** 2026-06-18
**Status:** Approved-pending-review
**Scope owner:** Bilal T
**Branch context:** `feat/tray-overlay-unify`

## Goal

Put Pairwire in a colleague's hands as a downloadable macOS application: a `.dmg` they
double-click, grant Screen Recording + Microphone, and immediately start a 1:1 screen-share
+ audio session via a pasted invite link — backed by our own relay we can monitor and
cost-cap, with opt-in telemetry so we get signal when something breaks.

v1 is **packaging and distribution only**. No new product features. The four packaging
workstreams the owner listed (DMG, website, telemetry, self-hosted relay) plus a minimal
update check.

## Current State (research-grounded)

- **One binary** `pairwire` in `crates/app` (`pairwire host` / `pairwire join <invite-url>`).
- **UI:** egui 0.31 + eframe 0.31 (wgpu). Transparent fullscreen overlay. **No webview.**
  Entry: `run_host_with_overlay_and_tray()` at `crates/app/src/main.rs:139`.
- **Transport:** iroh 0.97 QUIC + moq-lite 0.15. Currently uses **iroh's public relays**
  (no custom relay wired).
- **Capture:** scap `0.1.0-beta.1` (ScreenCaptureKit). Calls `request_permission()` at runtime
  (`crates/capture/src/capture_thread.rs:47-57`).
- **Input:** CGEvent injection via core-graphics — needs Accessibility (`crates/input/src/inject_macos.rs`).
- **FFmpeg:** `ffmpeg-next` 8, **dynamically linked to system FFmpeg** via pkg-config / `FFMPEG_DIR`.
  Users today would need `brew install ffmpeg`. Encode/decode behind `encode`/`ffmpeg` features.
- **Audio:** Opus 0.3.1 always present; cpal 0.17 capture/playback behind `audio` feature.
- **Packaging infra:** none. No `.app`, no `Info.plist`, no `.entitlements`, no `.icns`
  (tray icon is a generated 16×16 checkerboard at `crates/app/src/tray.rs:117-126`).
  `justfile` has no package/sign/notarize recipes.
- **CLI:** hand-rolled parser in `cli.rs`. No `--version`, no `--help`.
- **No update / telemetry / crash-report hooks** anywhere.

## Decisions (locked)

| Decision | Choice | Consequence |
|---|---|---|
| Signing | **Unsigned (ad-hoc) DMG is v1.** No Apple enrollment. | Build pipeline signing-agnostic so a Developer ID identity flips in later with one config change. Site documents Gatekeeper "Open Anyway". |
| FFmpeg | **Static-first**, dylibbundler fallback, decided in-build by a VideoToolbox spike. | If static FFmpeg loses VideoToolbox HW accel, switch to bundling+rpath-rewrite+per-dylib ad-hoc sign. |
| Telemetry region | **EU** (PostHog EU + Sentry EU). | GDPR-safe for EU testers; immutable choice. |
| Updates | **Minimal launch version-check.** | No in-place auto-install in v1. |
| Domain | **pairwire.dev** | Matches `api.pairwire.dev` naming in docs/12. `relay.pairwire.dev` for the relay. |
| Relay | **Own Hetzner CX22 (EU) at launch.** | Full cost/traffic visibility day 1; ops dependency accepted. |
| Product scope | **Packaging only.** QoL items are fast-follow. | Persistent identity / 6-digit codes / terminal / tunneling all out. |

## Why unsigned is workable for v1

The fragility of unsigned apps (TCC permission grants keyed to code-signing identity, reset
on every rebuild) bites **the developer during dev**, not end users. An end user downloads a
**fixed artifact** whose ad-hoc signature is stable for that binary, so their granted
permissions persist. The remaining friction is Gatekeeper: macOS Sequoia removed the
Control-click→Open bypass, so first launch requires **System Settings → Privacy & Security →
Open Anyway → admin password**. The website documents this.

We still ad-hoc codesign (`codesign --sign -`) the `.app` so hardened-runtime entitlements and
`Info.plist` usage strings are internally consistent and the TCC prompts fire correctly. The
pipeline is identical to the signed path minus three terminal steps (Developer ID identity,
`notarytool submit`, `stapler staple`).

---

## Workstream A — macOS `.app` + unsigned DMG  (effort: L)

The single biggest item and a hard prerequisite for B/C/D distribution.

**Tooling:** `cargo-packager` (crabnebula-dev) — produces both `.app` and `.dmg`, accepts a
signing identity and entitlements path, and the steps it skips for unsigned (notarize/staple)
are exactly the ones we omit. Config-driven so Developer ID flips in later.

**Components:**
1. **Icons** — design a 1024×1024 PNG → `iconutil` → `pairwire.icns`; matching tray-icon PNG
   replacing the checkerboard at `crates/app/src/tray.rs:117-126`. *(Design task — needs an asset.)*
2. **`Info.plist`** — `CFBundleIdentifier` (`dev.pairwire.app`), version, min macOS `12.3`,
   `NSScreenCaptureUsageDescription`, `NSMicrophoneUsageDescription`.
   *(Missing screen-capture key = process killed on first ScreenCaptureKit call, no error.)*
3. **`pairwire.entitlements`** — hardened runtime, `com.apple.security.device.audio-input`,
   `com.apple.security.device.camera` (not needed — omit), JIT/unsigned-memory only if wgpu/FFmpeg require.
4. **FFmpeg bundling** — see spike below. Goal: clean machine runs with **no `brew install`**.
5. **Release build** — `--release` with `encode,capture,audio,gui,tray` features.
6. **ad-hoc codesign** the inner binaries bottom-up, then the `.app` with `--options runtime`.
7. **DMG** assembly + (later) Developer-ID sign / notarize / staple — stubbed but parameterized.
8. **CI release workflow** (`.github/workflows/release.yml`) on tag push → builds, packages,
   uploads `.dmg` to GitHub Releases. macOS runner (~$1.20-1.60/run, ~15-20 min).
9. **`justfile` recipes:** `package`, `dmg`, (`sign`, `notarize` as no-op-able stubs).
10. **CLI polish:** `--version` (drives the update check) and `--help`. Consider `argh`
    (lightweight) vs keeping hand-rolled — minimal addition.

### FFmpeg spike (decides static vs dylib)
Time-boxed: build a static FFmpeg, link `ffmpeg-next` against it (`CARGO_FEATURE_STATIC`),
confirm **VideoToolbox H264 encode + decode still function** in a packaged `.app` on a clean
machine. If it works → static (self-contained, smallest notarization surface). If VideoToolbox
needs dynamic framework linkage → **dylibbundler**: copy `.dylib`s into `.app/Contents/libs/`,
rewrite rpaths (`install_name_tool`), ad-hoc-sign each dylib (unsigned dylib = "mapped file has
no cdhash" Gatekeeper error). Report the outcome before committing the route.

**Risks:** VideoToolbox/static interaction (above); malformed plist termination; scap pinned to
a beta; verify with `codesign -d --entitlements - -vvv` and `spctl --assess` before tester builds.

---

## Workstream B — Self-hosted iroh relay  (effort: M)

**Server:** Hetzner CX22 (EU), ~€4/mo, 20 TB/mo egress included. Run the **`iroh-relay` 0.97.0**
standalone binary (version-matched to the pinned `iroh 0.97.0` in Cargo.lock) with
`enable_metrics=true` (Prometheus-compatible).
TLS via the relay's built-in ACME or a fronting reverse proxy at `relay.pairwire.dev`.

**Client wiring + relay selection.** A single `resolve_relay_mode()` in the transport crate
decides the `RelayMode` with this precedence (highest first):
1. `PAIRWIRE_RELAY_URL=<url>` → `RelayMode::Custom(vec![url])` — explicit override, any build.
2. `PAIRWIRE_RELAY=public` → `RelayMode::Default` (n0's public relays) — the dev/internal toggle.
   `PAIRWIRE_RELAY=disabled` → `RelayMode::Disabled` (direct-only). `PAIRWIRE_RELAY=ours` →
   our relay (same as the release default).
3. **Build-profile default (no env set):**
   - **Debug builds** (`cfg!(debug_assertions)`) default to `RelayMode::Default` (n0 public) — so
     `cargo run` and debug DMGs dogfood on n0 without standing up our relay.
   - **Release builds** default to `RelayMode::Custom(vec![relay.pairwire.dev])` — **our relay
     only, no n0 in the map.** Shipped end-user traffic never touches n0's public network.

Rationale (owner directive, 2026-06-18): we may use n0's public relay *internally* but must not
route *end-user* traffic through it. The debug/release split enforces this automatically; the env
vars give explicit control (and let an operator opt into n0 as a fallback if ever desired). Default
release has **no n0 fallback** — a relay outage degrades to direct-only (hole-punched) sessions,
which is the honest trade for not free-riding n0. Validate the URL at the boundary; reject malformed.

**Monitoring + cost-cap:**
- Grafana Cloud free tier (10k metrics) scrapes the relay's Prometheus endpoint.
- **Cost-cap knob:** `iroh-relay` has **no per-node rate limiting yet** (issue #2967). Use
  Hetzner firewall rules + an **egress alert at 15 TB/mo**. The relay has no auth by default —
  either restrict inbound at the firewall or accept open-relay risk during early beta and watch
  the egress dashboard.

**Infrastructure as code (Terraform):** all relay infra lives in `infra/terraform/`, reproducible
and tear-down-able — no click-ops on Hetzner/Cloudflare.
- **Providers:** `hetznercloud/hcloud` (server, firewall, ssh key) + `cloudflare/cloudflare`
  (DNS records, optionally the Pages project). Optional `jianyuan/sentry` for the Sentry project.
- **Resources:** `hcloud_server` (CX22, EU, IPv4+IPv6), `hcloud_firewall`
  (**inbound allow: TCP 443, UDP 7842, SSH 22 from admin IP only; default-deny**),
  `hcloud_ssh_key`; `cloudflare_record` for `relay.pairwire.dev` (A + AAAA).
- **Server provisioning:** `cloud-init` (`user_data`) installs the `iroh-relay` binary, writes a
  `systemd` unit, configures ACME TLS for `relay.pairwire.dev`, enables Prometheus metrics.
- **State:** Terraform Cloud free tier *or* a Cloudflare R2 backend (keep state out of git;
  secrets via `TF_VAR_*` env / GitHub Actions secrets, never committed).
- **Wrapper:** `just relay-plan` / `just relay-apply` / `just relay-destroy`. Optional CI
  `terraform plan` on PR.
- **Note:** PostHog has no official TF provider — its project setup stays a manual task. The
  egress cost-cap *alert* is a Grafana concern (not TF); the firewall IP allowlist *is* TF.

**Cost reality:** a 1-hour relayed coding session at ~600 kbps ≈ €0.0003. Even fully-relayed
beta traffic stays far inside the 20 TB allowance. Relay cost is **not** a concern at launch
scale; the value is visibility and the cap mechanism.

**Risks:** open relay abuse (mitigate via firewall/egress alert); verify iroh 0.97 does **not**
carry the v0.96 hole-punch regression noted in LEARNINGS.md (slow path stays on relay forever
after path loss) — test reconnect on real LTE.

---

## Workstream C — pairwire.dev website  (effort: S)

- **Domain:** register `pairwire.dev` (~$12/yr, Cloudflare Registrar supports `.dev`).
  Nameservers → Cloudflare (free DNS + TLS). Subdomain `relay.pairwire.dev` → Hetzner relay.
- **Landing + download:** static site on **Cloudflare Pages** (free, unlimited bandwidth,
  commercial OK). DMG artifact hosted on **GitHub Releases** (free, no bandwidth cap).
- **Must state:** macOS 12.3+ requirement; Screen Recording + Microphone grant flow (with
  screenshots); the Gatekeeper **"Open Anyway"** steps for the unsigned build; an honest
  "unsigned beta" note.
- **`/version.json`** endpoint (static file) for the update check (Workstream E).

**Risks:** low. Keep the page honest about unsigned status to preserve trust.

---

## Workstream D — Opt-in telemetry  (effort: M)

**Stack:** `posthog-rs` (usage events) + `sentry` (crash + panic hook) + `uuid` (anonymous
device ID generated once) + `dirs` (config dir). **EU endpoints** for both.

**Consent model (GDPR Art. 6(1)(a)):**
- First-run consent dialog shown **after the first endpoint is live** (not at cold launch).
- Everything gated behind a process-global `AtomicBool` checked before any event fires.
- Anonymous `uuid` persisted to OS config dir; **no PII, no NodeIds**.

**Allowlist (collect):** connection success/failure, session duration, app version, OS family,
panic/crash stack traces.
**Denylist (never):** screen content, audio, keystrokes/input, invite tokens, NodeIds, IPs
beyond what the transport already exposes.

**Hardening:** Sentry DSN ships embedded in the binary → enable rate-limiting + abuse scrubbing
on the Sentry project. Pin `sentry` to an exact version (pre-1.0, breaks on minor bumps).
Document a Legitimate-Interests stance only if we ever decouple crash reporting from consent;
for v1, **crash reporting honors the same opt-in** to keep the consent story simple.

**Config foundation:** introduces a minimal config file (consent bool + device uuid) in the OS
config dir via `dirs` + TOML. This is a small slice of the larger P4.7 config system — keep it
to those two keys for v1.

**Risks:** consent logic must be airtight; pre-1.0 sentry churn; embedded DSN abuse.

---

## Workstream E — Minimal update version-check  (effort: S)

On launch: GET `https://pairwire.dev/version.json`, compare to the binary's `--version` (semver).
If newer → non-blocking notification ("Update available → download") linking to the download
page. **No in-place install** in v1. Wire into the existing tray/overlay notification surface.

**Risks:** low. Network call must be non-blocking and fail-silent.

---

## Sequencing & dependencies

```
A (DMG pipeline)  ──┬─> produces the artifact C links to, E checks version of
                    │
B (relay)  ─────────┼─> independent; client wiring (RelayMode::Custom) lands in the
                    │   same release build A packages
C (website) ────────┼─> needs domain (parallel) + A's DMG URL + B's relay subdomain
                    │
D (telemetry) ──────┴─> independent code; consent dialog uses app UI; ships in A's build

E (update-check) ──> needs C's /version.json; trivial client code in A's build
```

**Parallelizable now:** A's icon/plist/entitlements scaffolding, B's server provisioning,
C's domain registration + landing page, D's telemetry crate. **The FFmpeg spike gates A's
final form** and should run first. **Convergence point:** the release build that A packages
must include B's relay wiring, D's telemetry, and E's version-check.

## Testing strategy

- **A:** packaged `.app` runs on a **clean macOS VM/Mac** (no Homebrew FFmpeg) — capture,
  encode, audio, input all functional. `codesign -d` + `spctl --assess` verification gates.
- **B:** relay up, `RelayMode::Custom` forces traffic through it; confirm a relayed session
  works and Prometheus metrics increment; test reconnect on LTE (hole-punch regression check).
- **C:** download → Open Anyway → grant permissions → join a session end-to-end on a fresh Mac.
- **D:** consent off = zero events emitted (verify network); consent on = allowlisted events
  only, no denylisted data in payloads; panic produces a Sentry event.
- **E:** stale version triggers notification; current version is silent; offline fails silently.

## Out of scope (fast-follow / later specs)

- **QoL fast-follow** (mostly half-built already — cheap): host-tray **mute toggle**
  (`AudioCaptureThread.muted_flag()` exists at `capture.rs:26-55`, host never wires it);
  **connection-quality popover** (`ConnectionHealth{rtt_ms,loss_per_mille}` computed every 1s
  in `quality.rs` then discarded — expose the watch channel); **reconnecting banner in viewer**
  (`reconnect.rs` state machine + host banner pattern exist); **who-joined toast** (host already
  shows 8-char EndpointId).
- **Persistent identity** — needs its own spec (iroh `SecretKey` persistence to config dir;
  prerequisite for 6-digit codes and stable "who is this person"). EndpointId currently
  regenerates every `host` run.
- **6-digit rotating codes** (rendezvous server, docs/12), **persistent audio across share**
  (XL session.rs refactor, P4.9), **terminal sharing** (P4.1-4.3, crate is a stub),
  **localhost tunneling** (P4.4-4.6), **full auto-update**, **Developer ID + notarization**
  (flip-in when/if enrollment happens).

## Open items for the owner (non-blocking)

- **License:** workspace is MIT OR Apache-2.0 (`Cargo.toml:18`). docs/12 recommends **BSL 1.1**
  before any public release to protect the hosted relay/rendezvous from free-riding. Decide
  before the download page goes public. Confirm no external-contributor consent needed.
- **Bundle identifier** `dev.pairwire.app` — confirm naming.

## Manual / human-in-the-loop tasks

Agents cannot create paid accounts, buy domains, accept ToS, or hold secrets. These are
**yours**, and each one unblocks downstream agent work. Do them first / in parallel.

| # | Manual task | Unblocks | Notes |
|---|---|---|---|
| 1 | Register **pairwire.dev** | C (site), B (relay subdomain), E (version-check URL) | Cloudflare Registrar supports `.dev`, ~$12/yr. Payment required. |
| 2 | Create **Hetzner Cloud** account + project + **API token** | B (Terraform `hcloud`) | Provide `TF_VAR_hcloud_token`. Card required. |
| 3 | Create **Cloudflare** account + scoped **API token** (DNS edit + Pages) | B (DNS), C (Pages) | `TF_VAR_cloudflare_token`. Free. |
| 4 | Provide an **SSH public key** for the relay server | B (server access) | Goes into `hcloud_ssh_key`. |
| 5 | Create **PostHog Cloud (EU)** project → **project API key** | D (telemetry) | Free tier. No TF provider — manual. |
| 6 | Create **Sentry (EU)** project → **DSN** | D (crash reporting) | Free dev plan. Can be TF'd (`jianyuan/sentry`) or manual. |
| 7 | Choose **state backend secret** (Terraform Cloud token *or* R2 keys) | B (IaC state) | Keeps TF state out of git. |
| 8 | Decide **license** (MIT/Apache → BSL 1.1?) before site goes public | C (public release) | Owner call; non-blocking for build. |
| 9 | **App icon asset** (1024×1024 PNG) — design or commission | A (.icns + tray icon) | Only true creative blocker in A. |

**Secrets handling:** store tokens in a password manager + GitHub Actions secrets. Never commit.
The Sentry DSN *is* embedded in the release binary (unavoidable) — enable rate-limiting + abuse
scrubbing on the Sentry project to compensate.

**Agent vs human boundary:** agents author all code, Terraform configs, CI workflows, cloud-init,
the landing page, and the telemetry/relay wiring. Humans run `terraform apply` (or approve the CI
apply), perform account creation + payment above, and supply the icon asset. The plan will tag
each task `[human]` or `[agent]` so the split is explicit.

## Cost summary (launch scale)

| Item | Cost |
|---|---|
| Apple Developer | **$0** (skipped — unsigned v1) |
| pairwire.dev domain | ~$12/yr |
| Cloudflare Pages | $0 |
| GitHub Releases (DMG) | $0 |
| Hetzner CX22 relay | ~€4/mo (20 TB egress incl.) |
| PostHog Cloud (EU) | $0 (1M events/mo free) |
| Sentry (EU) | $0 (5k errors/mo free) |
| GitHub Actions macOS | ~$1.20-1.60 per release |
| **Total** | **~€4-5/mo + ~$12/yr** |
