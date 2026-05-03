# Pairwire Open-Core Strategy & v1 Implementation Plan

## Context

Pairwire today is a technically strong, single-binary 1:1 P2P pair-programming tool (iroh + moq-lite + FFmpeg + Opus). It works. What it doesn't have is a product shape: no accounts, no address book, no billing, no short codes, no team concept, no monetization. The current onboarding asks the host to copy-paste a 100+ character `pairwire://<hex-key>?relay=…&token=…` URL into chat — that's a developer-demo UX, not a product UX.

The goal of this change is to draw the open-core boundary cleanly **before** writing the SaaS code, so we don't accidentally bake decisions that close doors. Specifically: be honest about where the moat actually lives (the technical core, not the cloud features), pick a license that protects it, and replace the long-URL flow with a short OTP-style code that resolves through a rendezvous service.

The buyer is currently unknown — solos vs small teams vs enterprise. v1 is designed to preserve optionality: a source-available funnel (anyone can read, hack, self-host pairwire), the hosted free tier requires only a free account (so we can talk to users + rate-limit abuse), and the paid surface stays small enough to pivot once we see who shows up.

---

## Where the moat actually is

An earlier draft of this plan assumed the closed multi-tenant cloud (accounts, address book, billing, seats) was the moat, with the OSS client as adoption funnel. **That assumption no longer holds in 2026.** With AI-assisted development, "Resend + Auth.js + Stripe + a contacts table" is a weekend's work, not a year's. Auth, billing, CRUD, and team directory are increasingly cookie-cutter — not what a competitor would struggle to replicate.

What *is* hard to replicate is the technical core that already exists in this repo:

- iroh `Endpoint` integration with NAT traversal and relay fallback (`crates/transport/`)
- moq-lite track plumbing for video and audio with keyframe-aligned groups (`crates/transport/`, `crates/media/`)
- scap capture quirks: Retina vs logical resolution, idle empty frames, threading constraints (`crates/media/`, see `LEARNINGS.md`)
- FFmpeg encode/decode pipeline with H.264 keyframe gating on the decode side
- `UniStreamRouter` for multiplexing moq-lite and raw screen streams over a single QUIC connection (`crates/transport/src/stream_router.rs`)
- Cursor + remote-control wire protocol with QUIC datagrams under the 1200-byte MTU
- Opus audio with cpal across platforms

This is months of specialized iroh / moq / scap / FFmpeg domain knowledge, baked into a working binary. It's the actual differentiation. The license needs to protect *this*, not the SaaS layer.

The cloud features still need to exist — you can't sell a hosted product without auth and billing — but they're operational table stakes, not what protects the business.

---

## Open vs Closed Boundary

| Layer | Status | Repo |
|---|---|---|
| `pairwire` executable (client) | **Source-available** (BSL — see below) | `pairwire/` (this repo) |
| Wire protocol code (`crates/protocol`) | **Source-available** (BSL) | `pairwire/` |
| Reference rendezvous server (1:1, in-mem, anon, rate-limited) | **Source-available** (BSL) | `pairwire/` |
| Production rendezvous service (auth-gated, persistent, observability) | **Closed** (table stakes) | `pairwire-cloud/` (new private repo) |
| Account / signup / sessions | **Closed** (table stakes) | `pairwire-cloud/` |
| Personal address book (v1) | **Closed** (table stakes) | `pairwire-cloud/` |
| Billing / Stripe | **Closed** (table stakes) | `pairwire-cloud/` |
| Team directory / seats / SSO (v1.1+) | **Closed** | `pairwire-cloud/` |
| Multi-party rooms (PLAN.md P5.1) | **Deferred** | TBD |

The split is unchanged from the earlier draft, but the *reasoning* is different. Cloud is closed because it has to run somewhere we control (auth, billing, multi-tenant data). It is **not** the moat — the source-available client is. BSL on the client preserves the OSS-style funnel (read it, hack it, self-host it, audit it) while preventing a competitor from packaging the technical core into a competing hosted offering.

---

## License Decision (NEEDS YOUR PICK)

You currently have `MIT OR Apache-2.0` in the workspace `Cargo.toml`. Given where the moat actually lives, MIT/Apache hands away the protected work. The recommendation has flipped from the earlier draft: **adopt a source-available license with a non-compete clause.**

### Option A — Business Source License (BSL 1.1) — Recommended

Convert the workspace to BSL 1.1 with a 4-year change date that auto-converts to Apache-2.0.

- **Use grant**: any non-production or non-competing use. Self-host for your own team, fork to study, contribute back, even use commercially internally — as long as you're not running "Pairwire-as-a-Service" against us.
- **Pro**: protects the technical moat directly. A competitor can't fork `crates/transport/` + `crates/media/` and run a competing hosted offering during the change-date window.
- **Pro**: still source-available — full read/hack/self-host funnel for individuals, hobbyists, security reviewers. Avoids the "open client, mystery server" anti-pattern.
- **Pro**: precedent — HashiCorp (Terraform/Vault), Sentry, CockroachDB, Couchbase, MariaDB, Materialize. All source-available, all with profitable hosted services and no fork-and-undercut competitor.
- **Pro**: auto-converts to Apache after the change-date window, so it's not permanently restrictive — just protected during the period when forks would actually hurt. By year 4, either pairwire is a real business with brand + distribution as moats (source can safely open) or it isn't (restriction was moot).
- **Con**: not OSI-approved, so it's not "open source" by strict definition. Some communities will treat it that way; some won't.
- **Con**: relicensing requires either contributor consent (currently just you, easy) or a CLA going forward.

### Option B — Functional Source License (FSL)

Sentry's license. Similar shape to BSL but simpler text — non-compete for 2 years, then auto-converts to MIT or Apache.

- **Pro**: shorter restriction window (2 years vs 4); slightly more community-friendly framing.
- **Pro**: well-documented, designed by Sentry explicitly for the "open-core company protecting itself" use case.
- **Con**: newer than BSL, less legal-team familiarity at large companies.

### Option C — AGPL-3.0

Strong copyleft with a network-use clause.

- **Pro**: real, OSI-approved open-source license. No "is this really open source?" debate.
- **Pro**: anyone running pairwire as a service has to release their modifications.
- **Con (reframed)**: large enterprises blanket-ban AGPL. The earlier draft called this a problem; the honest version is that it's *also* a feature — companies that can't accept AGPL are exactly the ones you'd want paying for a commercial license. But it does shut down the OSS-into-enterprise organic-adoption path.
- **Con**: weaker protection than BSL/FSL against the specific threat. AGPL only forces release of modifications; a competitor could still run a hosted pairwire as long as they publish their fork.

### Option D — Fully closed-source client

Ship pairwire as a closed-source binary. Publish the wire protocol spec so compatible clients are theoretically possible, but the implementation stays proprietary.

- **Pro**: maximum moat protection; total control of distribution.
- **Pro**: open the source later, once brand + distribution have become the moat instead of code.
- **Con**: loses the source-available funnel entirely. No community security review, no contributor pipeline, no "this is hackable" trust signal.
- **Con**: weaker hiring story — "come work on our closed-source video codec glue" is less attractive than "come work on our source-available P2P stack."
- **Con**: works against the "P2P, no servers in the data path, you can verify it" trust narrative that's a natural marketing angle for this product.

### Recommendation: BSL 1.1 (Option A)

It fits the actual situation: technical core is the moat, you want to protect it for the next ~4 years while the business is forming, you don't want to permanently restrict community use or close the contributor pipeline. Auto-conversion to Apache is the right pressure-release valve.

Pick FSL if you want a shorter restriction window. Pick AGPL if you specifically want to *force* an enterprise commercial-license conversation. Pick closed-source if you'd rather not be source-available at all yet (a defensible choice — just commit to it deliberately rather than as a default).

---

## v1 Product Shape

### Short codes (replaces `pairwire://` URLs as the default)

The client supports **three code formats** (server picks; client accepts any):

- `483-291` — 6-digit numeric, dash for readability (~1M keyspace, time-bound 10min, MUST be online-rendezvous + per-IP rate-limited)
- `k4p-9xrm` — 8-char alphanumeric (~2T keyspace)
- `quiet-otter-river` — three words from a curated wordlist (~64B keyspace, read-aloud-friendly)

Default issuance format is configurable on the rendezvous server side. The legacy `pairwire://` long URL remains supported as a fallback (`--legacy-url` to issue, auto-detected on join).

### Free tier (account-required, no payment)

- Email magic link OR GitHub OAuth signup.
- Anonymous use of the public hosted rendezvous is **not** allowed (this is the trade for abuse control + an upsell channel). Self-hosters can run their own BSL-licensed rendezvous with whatever auth model they want.
- Free tier gets: short codes, personal address book (recent peers), unlimited 1:1 sessions.

### Paid tier (v1 = address book + seats; v1.1+ = team directory + SSO)

- Personal address book ships in v1 free; team-org features are the paid hook.
- Stripe scaffolding lands in v1 but with no live SKU yet — pricing decided once we see who signs up.
- Reminder: the paid layer's job is to **capture revenue from users who already chose pairwire because the core is good** — not to be the differentiator itself.

### Self-host story

- Repo includes a thin reference rendezvous server (`crates/rendezvous-server`): in-memory store, per-IP rate limit, no auth, supports all three code formats. BSL-licensed like the rest of the workspace.
- Client takes `--rendezvous <url>` (or `PAIRWIRE_RENDEZVOUS` env) — defaults to `api.pairwire.dev`. Self-hosters point at their own.
- Production cloud rendezvous (closed) is a drop-in API-compatible replacement that adds auth, persistence, observability, and address-book integration.

---

## Implementation Steps

### Phase 0 — Relicense (this repo)

0. Update workspace `Cargo.toml` `license` field to chosen SPDX identifier (e.g., `BUSL-1.1`).
1. Replace root `LICENSE` (currently MIT/Apache dual) with the chosen license text. For BSL, include the Additional Use Grant and Change Date headers.
2. Update README license section. Add a short "Why source-available?" paragraph that explains the change for current/future contributors.
3. If a CONTRIBUTING.md exists or gets added, note the licensing terms for new contributions.

### Phase 1 — Protocol + code formats

4. **`crates/core/src/code.rs`** (new): `Code` type with three encoders/decoders (numeric / alpha / phrase). Curated 4096-word wordlist embedded.
5. **`crates/protocol/src/rendezvous.rs`** (new): wire types for `IssueCodeRequest`, `IssueCodeResponse`, `ResolveCodeRequest`, `ResolveCodeResponse`. Versioned per `CONVENTIONS.md`.
6. **`crates/transport/src/rendezvous.rs`** (new): HTTP client wrapping the protocol, with retry + timeout.
7. **`crates/transport/src/invite.rs`**: extend `InviteLink` to also carry a `Code` variant alongside the existing URL form. Reuse the existing `EndpointAddr`/`SessionId` plumbing — don't duplicate.

### Phase 2 — Reference rendezvous server

8. **`crates/rendezvous-server/`** (new crate, new binary `pairwire-rendezvous`): axum + in-memory `DashMap`, per-IP rate limit (check Cargo.toml before adding `governor`), TTL eviction, all three code formats. Health endpoint. No auth, no persistence.
9. Add to workspace `Cargo.toml`. Add `just rendezvous` recipe to the Justfile.

### Phase 3 — Wire it into the client

10. **`crates/app/src/cli.rs`**: add `--rendezvous <url>` (env: `PAIRWIRE_RENDEZVOUS`, default `api.pairwire.dev`), `--legacy-url` flag on `host`, `--code-format {numeric|alpha|phrase}` flag.
11. **`crates/app/src/session.rs:54-76`**: replace `transport.invite_link(session_id)` direct print with: issue code via rendezvous, print code, fall back to printing long URL on rendezvous failure (with a clear warning).
12. **`crates/app/src/cli.rs` join path**: parse arg as `Code` first, fall back to `pairwire://` URL parse. On code, resolve via rendezvous to get `EndpointAddr` + `SessionId`, then existing connect path.
13. **`README.md`** + relevant `docs/`: update onboarding to short-code flow. Add `docs/13-rendezvous-protocol.md` spec doc for self-hosters.

### Phase 4 — Scaffold the closed cloud repo (separate private repo)

Skeleton only — actual product work continues after the source-available work is shipped.

14. New repo `pairwire-cloud/` (private). Recommend: TypeScript + Hono on Cloudflare Workers + D1, or Rust + axum on Fly.io. Decide based on your familiarity — this is a velocity question, not a tech-superiority one.
15. Stub: `/v1/codes` (POST issue, GET resolve) — same wire protocol as the OSS server, with auth middleware added.
16. Stub: account model (email magic link via Resend; GitHub OAuth via standard flow).
17. Stub: address book table + `/v1/peers` endpoint.
18. Stripe scaffolding: customer create on signup, no SKU live yet.
19. CI: deploy on push to main, env-driven config, secrets in 1Password Connect or Doppler.

### Phase 5 — Glue: client knows about cloud accounts

20. **`crates/app/src/cli.rs`**: `pairwire login` (browser-based OAuth-style flow that drops a token in `~/.config/pairwire/auth.toml`).
21. **`crates/transport/src/rendezvous.rs`**: bearer-auth header when token present.
22. Address book sync: after a session, optionally POST peer info to cloud (with explicit consent — privacy-first; default off and clearly labeled).

---

## Critical Files (to modify or create)

- `Cargo.toml` (workspace) — change `license` from `MIT OR Apache-2.0` to chosen SPDX (`BUSL-1.1` if BSL)
- `LICENSE` (root) — replace dual-license content with chosen license text + Additional Use Grant + Change Date for BSL
- `README.md` — license section + a short "why source-available" note
- `crates/transport/src/invite.rs:23-56` — extend, don't replace
- `crates/transport/src/transport.rs:25,49` — keep iroh n0 relay preset; rendezvous is a separate concern from NAT-traversal relays (different services)
- `crates/app/src/cli.rs:10-16` — extend subcommands
- `crates/app/src/session.rs:54-76` — main wiring change
- `crates/protocol/src/lib.rs` — register new `rendezvous` module
- `crates/core/src/types.rs` — add `Code` type
- New: `crates/rendezvous-server/` (full crate)
- New: `docs/13-rendezvous-protocol.md`
- New repo: `pairwire-cloud/` (separate, private)
- `LEARNINGS.md` — log iroh/rate-limit/code-format gotchas as we hit them
- `PLAN.md` — add a P3.7 "Rendezvous & short codes" section between P3 and P4; add a P0 "Relicense to BSL" task at the top

---

## What's Explicitly Deferred (NOT in v1)

- Multi-party rooms / moq-relay fan-out (PLAN.md P5.1 stays deferred — needs the team buyer signal first)
- Recording / transcription
- SSO, audit logs, on-prem cloud
- Team directory (only personal "recent peers" address book in v1)
- Live pricing — Stripe wired but no SKU until we know the buyer
- Brand split (single-brand for now; renameable later)

---

## Verification

End-to-end sanity for the source-available work:

```bash
# 1. Workspace builds clean
just all

# 2. Run reference rendezvous locally
just rendezvous &  # listens on :8080

# 3. Host issues a short code via local rendezvous
PAIRWIRE_RENDEZVOUS=http://localhost:8080 cargo run -p pairwire -- host
# expect: prints "Code: 483-291" (or chosen format)

# 4. From a second machine (or second terminal with different identity dir):
PAIRWIRE_RENDEZVOUS=http://localhost:8080 cargo run -p pairwire -- join 483-291
# expect: connects, video appears

# 5. Legacy URL still works
cargo run -p pairwire -- host --legacy-url
cargo run -p pairwire -- join "pairwire://..."
# expect: same behavior as today

# 6. Rate-limit works
for i in {1..200}; do curl -X POST localhost:8080/v1/codes; done
# expect: 429s after ~100 (configurable)

# 7. All three code formats round-trip
cargo test -p pairwire-core code::

# 8. License sanity
grep -r "MIT OR Apache" Cargo.toml crates/  # expect: no matches after Phase 0
head -5 LICENSE  # expect: BSL header (or chosen license)
```

For Phase 4–5 (cloud): separate verification in the `pairwire-cloud` repo — out of scope for this plan file.

---

## Open Questions (for you to confirm)

1. **License**: BSL 1.1 (recommended), FSL, AGPL, or fully closed-source?
2. **BSL Additional Use Grant scope** (if BSL): standard wording is "may make production use of the Licensed Work, provided that you do not offer the Licensed Work to third parties on a hosted or embedded basis in order to compete with Pairwire's paid version." OK as-is or tighter/looser?
3. **Change date** (if BSL): 4 years (BSL standard) or shorter?
4. **Default hosted rendezvous URL** in the binary: `api.pairwire.dev` (or different domain)? Acceptable for the binary to ship pointing at your hosted service by default? (Self-host overridable via flag/env.)
5. **Cloud stack pick**: TypeScript + CF Workers, or Rust + axum + Fly? Doesn't change this plan, but unblocks Phase 4 once chosen.
6. **Existing contributors**: any external contributors to relicense? (If just you, Phase 0 is a unilateral commit. If others, need consent or CLA.)
