# Pairwire Open-Core Strategy & v1 Implementation Plan

## Context

Pairwire today is a technically strong, single-binary 1:1 P2P pair-programming tool (iroh + moq-lite + FFmpeg + Opus). It works. What it doesn't have is a product shape: no accounts, no address book, no billing, no short codes, no team concept, no monetization. The current onboarding asks the host to copy-paste a 100+ character `pairwire://<hex-key>?relay=…&token=…` URL into chat — that's a developer-demo UX, not a product UX.

The goal of this change is to draw the open-core boundary cleanly **before** writing the SaaS code, so we don't accidentally bake decisions that close doors. Specifically: keep the protocol + executable open and self-hostable, build the team-workflow layer (accounts, address book, billing, seats, eventually SSO) as a closed service, and replace the long-URL flow with a short OTP-style code that resolves through a rendezvous service. Self-hosters can run the OSS reference rendezvous; the hosted version adds the team layer.

The buyer is currently unknown — solos vs small teams vs enterprise. v1 is designed to preserve optionality: the OSS funnel is maximized (anyone can run pairwire), the hosted free tier requires only a free account (so we can talk to users + rate-limit abuse), and the paid surface stays small enough to pivot once we see who shows up.

---

## Open vs Closed Boundary

| Layer | Status | Repo |
|---|---|---|
| `pairwire` executable (client) | **Open source** | `pairwire/` (this repo) |
| Wire protocol (`crates/protocol`) | **Open source** | `pairwire/` |
| Reference rendezvous server (1:1, in-mem, anon, rate-limited) | **Open source** | `pairwire/` |
| Production rendezvous service (auth-gated, persistent, observability) | **Closed** | `pairwire-cloud/` (new private repo) |
| Account / signup / sessions | **Closed** | `pairwire-cloud/` |
| Personal address book (v1) | **Closed** | `pairwire-cloud/` |
| Billing / Stripe | **Closed** | `pairwire-cloud/` |
| Team directory / seats / SSO (v1.1+) | **Closed** | `pairwire-cloud/` |
| Multi-party rooms (PLAN.md P5.1) | **Deferred** | TBD |

**Why this split is the recommendation**: the OSS reference rendezvous makes the protocol fully implementable (no "open client, mystery server" problem) and lets paranoid self-hosters run the whole 1:1 stack. The closed cloud service adds the team workflow on top — which is what people will actually pay for and which is genuinely hard to fork (it's a multi-tenant SaaS, not a binary).

---

## License Decision (NEEDS YOUR PICK)

You currently have `MIT OR Apache-2.0` in the workspace `Cargo.toml`. Three viable options, ranked by my recommendation strength:

### Option A — Keep MIT OR Apache-2.0 (Recommended)

Leave the OSS license as-is. Closed cloud code is just closed (no public license at all).

- **Pro**: maximum adoption funnel; zero enterprise license friction; honest — the moat is the closed multi-tenant service, not a copyleft fence.
- **Pro**: AGPL's network-use clause is designed for *services* (someone running your code as a SaaS). The pairwire client is a client — AGPL gives you almost no protection there that GPL wouldn't.
- **Con**: someone could in principle take the client + OSS rendezvous and build a competing hosted version. They'd still have to build the team workflow themselves, which is the actual work.

### Option B — Relicense client + reference server to AGPL

Switch this repo to AGPL-3.0.

- **Pro**: stronger forking deterrent; signals "we mean it" to the OSS community.
- **Con**: known enterprise repellent — many corporate legal teams blanket-ban AGPL. Forecloses enterprise as a buyer before we know if they are the buyer.
- **Con**: relicensing a dual-licensed project requires either contributor consent (currently just you, so easy) or a CLA going forward. One-way door.

### Option C — Functional Source License (FSL) on the reference server only

Keep the client MIT/Apache. License the reference rendezvous server under FSL (Sentry-style: non-compete for 2 years, then converts to Apache).

- **Pro**: prevents a hyperscaler from spinning up "pairwire rendezvous as a service" without the AGPL stigma.
- **Con**: more complex license story; FSL is newer and less familiar to legal teams; mixed-license repos are a slight maintenance burden.

**My recommendation: A.** The risk AGPL/FSL guards against (someone reselling our OSS) isn't real until we have meaningful users to capture, and by then we'll have built the closed team service that's the actual product. Don't mortgage adoption today against a hypothetical threat tomorrow.

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
- Anonymous use of the public hosted rendezvous is **not** allowed (this is the trade for abuse control + an upsell channel). Self-hosters can run their own OSS rendezvous with whatever auth model they want.
- Free tier gets: short codes, personal address book (recent peers), unlimited 1:1 sessions.

### Paid tier (v1 = address book + seats; v1.1+ = team directory + SSO)

- Personal address book ships in v1 free; team-org features are the paid hook.
- Stripe scaffolding lands in v1 but with no live SKU yet — pricing decided once we see who signs up.

### Self-host story

- OSS includes a thin reference rendezvous server (`crates/rendezvous-server`): in-memory store, per-IP rate limit, no auth, supports all three code formats.
- Client takes `--rendezvous <url>` (or `PAIRWIRE_RENDEZVOUS` env) — defaults to `api.pairwire.dev`. Self-hosters point at their own.
- Production cloud rendezvous (closed) is a drop-in API-compatible replacement that adds auth, persistence, observability, and address-book integration.

---

## Implementation Steps

### Phase 1 — Protocol + code formats (OSS, this repo)

1. **`crates/core/src/code.rs`** (new): `Code` type with three encoders/decoders (numeric / alpha / phrase). Curated 4096-word wordlist embedded.
2. **`crates/protocol/src/rendezvous.rs`** (new): wire types for `IssueCodeRequest`, `IssueCodeResponse`, `ResolveCodeRequest`, `ResolveCodeResponse`. Versioned per `CONVENTIONS.md`.
3. **`crates/transport/src/rendezvous.rs`** (new): HTTP client wrapping the protocol, with retry + timeout.
4. **`crates/transport/src/invite.rs`**: extend `InviteLink` to also carry a `Code` variant alongside the existing URL form. Reuse the existing `EndpointAddr`/`SessionId` plumbing — don't duplicate.

### Phase 2 — Reference rendezvous server (OSS, this repo)

5. **`crates/rendezvous-server/`** (new crate, new binary `pairwire-rendezvous`): axum + in-memory `DashMap`, per-IP rate limit (governor crate already common in the ecosystem — check before adding), TTL eviction, all three code formats. Health endpoint. No auth, no persistence.
6. Add to workspace `Cargo.toml`. Add `just rendezvous` recipe to the Justfile.

### Phase 3 — Wire it into the client (OSS, this repo)

7. **`crates/app/src/cli.rs`**: add `--rendezvous <url>` (env: `PAIRWIRE_RENDEZVOUS`, default `api.pairwire.dev`), `--legacy-url` flag on `host`, `--code-format {numeric|alpha|phrase}` flag.
8. **`crates/app/src/session.rs:54-76`**: replace `transport.invite_link(session_id)` direct print with: issue code via rendezvous, print code, fall back to printing long URL on rendezvous failure (with a clear warning).
9. **`crates/app/src/cli.rs` join path**: parse arg as `Code` first, fall back to `pairwire://` URL parse. On code, resolve via rendezvous to get `EndpointAddr`+`SessionId`, then existing connect path.
10. **`README.md`** + relevant `docs/`: update the onboarding section to show short-code flow. Add a `docs/11-rendezvous-protocol.md` spec doc for self-hosters.

### Phase 4 — Scaffold the closed cloud repo (separate private repo)

This is just the skeleton — actual product work continues after the OSS plan is shipped.

11. New repo `pairwire-cloud/` (private). Recommend: TypeScript + Hono on Cloudflare Workers + D1, or Rust + axum on Fly.io. Decide based on your familiarity — this is a Bilal-velocity question, not a tech-superiority one.
12. Stub: `/v1/codes` (POST issue, GET resolve) — same wire protocol as the OSS server, with auth middleware added.
13. Stub: account model (email magic link via Resend or similar; GitHub OAuth via standard flow).
14. Stub: address book table + `/v1/peers` endpoint.
15. Stripe scaffolding: customer create on signup, no SKU live yet.
16. CI: deploy on push to main, env-driven config, secrets in 1Password Connect or Doppler.

### Phase 5 — Glue: client knows about cloud accounts

17. **`crates/app/src/cli.rs`**: `pairwire login` (browser-based OAuth-style flow that drops a token in `~/.config/pairwire/auth.toml`).
18. **`crates/transport/src/rendezvous.rs`**: bearer-auth header when token present.
19. Address book sync: after a session, optionally POST peer info to cloud (with explicit consent — privacy-first; default off and clearly labeled).

---

## Critical Files (to modify or create)

- `Cargo.toml` (workspace) — confirm license string stays `MIT OR Apache-2.0` per Option A
- `crates/transport/src/invite.rs:23-56` — extend, don't replace
- `crates/transport/src/transport.rs:25,49` — keep iroh n0 relay preset; rendezvous is a separate concern from NAT-traversal relays (these are different services)
- `crates/app/src/cli.rs:10-16` — extend subcommands
- `crates/app/src/session.rs:54-76` — main wiring change
- `crates/protocol/src/lib.rs` — register new `rendezvous` module
- `crates/core/src/types.rs` — add `Code` type
- New: `crates/rendezvous-server/` (full crate)
- New: `docs/11-rendezvous-protocol.md`
- New repo: `pairwire-cloud/` (separate, private)
- `LEARNINGS.md` — log any iroh/rate-limit/code-format gotchas as we hit them
- `PLAN.md` — add a P3.7 "Rendezvous & short codes" section between P3 and P4

---

## What's Explicitly Deferred (NOT in v1)

- Multi-party rooms / moq-relay fan-out (PLAN.md P5.1 stays deferred — needs the team buyer signal first)
- Recording / transcription
- SSO, audit logs, on-prem cloud
- Team directory (only personal "recent peers" address book in v1)
- Live pricing — Stripe is wired but no SKU until we know the buyer
- Brand split (single-brand for now; renameable later)

---

## Verification

End-to-end sanity for the OSS work:

```bash
# 1. Workspace builds clean
just all

# 2. Run OSS rendezvous locally
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
```

For Phase 4–5 (cloud): separate verification in the `pairwire-cloud` repo — out of scope for this plan file.

---

## Open Questions (for you to confirm at exit-plan time)

1. **License**: Option A (MIT/Apache, my recommendation), B (AGPL), or C (FSL on the rendezvous)?
2. **Default hosted rendezvous URL** in the OSS binary: `api.pairwire.dev` (or different domain)? Acceptable for the OSS to ship pointing at your hosted service by default? (Self-host overridable via flag/env.)
3. **Cloud stack pick**: TypeScript+CF Workers, or Rust+axum+Fly? (Doesn't change this plan — but unblocks Phase 4 once chosen.)
