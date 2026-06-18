# Product

## Register

brand

## Users

Developers who pair-program. Two roles, one session:
- **The host** — wants to share their screen with a colleague *right now* to debug/pair, without scheduling a call, installing Zoom, or creating an account. Copies an invite link and sends it.
- **The joiner** — receives a link, opens it, sees the host's screen, talks, and draws on it.

Context: remote or hybrid devs, often mid-problem, low patience for setup. They reach for Pairwire the way they'd reach for a CLI tool — fast, local, no ceremony. The website's audience is this same developer, evaluating in ~10 seconds whether to download a rough experiment.

## Product Purpose

Pairwire is peer-to-peer screen-share + voice + draw-on-screen for pair programming. Direct device-to-device over QUIC (iroh), end-to-end encrypted, 1:1, no account, no server in the media path. macOS-first, unsigned beta.

The **site's** job: let a developer understand what it is, grasp roughly how it works, download the `.dmg`, and get through the macOS permission/Gatekeeper flow — all framed honestly as a weekend-energy experiment. Success = a dev downloads it, tries it with a colleague, and sends feedback. Not signups, not "conversions."

## Brand Personality

**Candid · technical · a little playful.** Voice is developer-to-developer: plain, terse, a touch self-deprecating, zero marketing. It says "this is rough and unsigned and might break — but it works, and it's genuinely useful, so try it." Credibility comes from engineering honesty, not claims. The one wink is the product's own motif: two screens joined by a looping wire (pair + wire), and the draw-on-screen feature. Emotional goal: *"oh, this is honest and actually works — let me try it."*

## Anti-references

Avoid all of these — they were explicitly rejected:
- **Generic AI landing**: tiny tracked uppercase eyebrows on every section, identical icon-card grids, cream/sand backgrounds, gradient text, numbered `01/02/03` section markers, hero-metric template.
- **Corporate SaaS**: "Trusted by" logo walls, testimonials, pricing tiers, gradient hero, feature-card matrix.
- **VC-startup slick**: over-polished launch gloss, aspirational stock imagery, big revolutionary claims.
- **Web3/crypto**: neon, 3D blobs, glassmorphism-everywhere, hype.

The trap to avoid: the chosen reference (Linear/Vercel) can slide into "startup slick." Take their *engineered restraint and precision*, not their launch-day gloss. Candor is what keeps it on the right side.

## Design Principles

1. **Honesty over hype.** State that it's an experiment, unsigned, rough. Candor is the differentiator and the whole positioning — never bury or soften it.
2. **Show, don't tell.** Demonstrate the thing (the connect/draw motif, a real screen, the actual invite-link flow) instead of describing benefits. No adjectives doing a screenshot's job.
3. **Engineered restraint.** Linear-grade precision: every element earns its place, generous negative space, exact alignment, no decorative scaffolding (eyebrows, card grids, fake sequences).
4. **Respect the reader's time.** A dev should *get it* and reach the download in seconds. Scannable, terse, front-loaded. The README energy is a feature.
5. **One wink, not a circus.** The looping-wire / draw motif is the single moment of play. Personality through one well-placed gesture, not constant whimsy.

## Accessibility & Inclusion

WCAG 2.1 AA. Body text ≥4.5:1 on the dark bg (periwinkle accent and muted text verified against the navy-black surface). Full keyboard operability; visible focus rings. Every animation (including the hero wire-connect motif) has a `prefers-reduced-motion: reduce` fallback (crossfade or static end-state). No meaning conveyed by color alone. The Gatekeeper/permissions instructions are real, legible text — not screenshots-only — so they're readable by screen readers.
