# Design

Visual system for the **pairwire.dev** landing/download site. Derived from `logo.png` (the committed brand identity). Identity-preservation wins — these tokens reproduce the logo's palette; sample exact hex from `logo.png` during build and tune the OKLCH values to match.

## Theme

**Dark.** Not a default — the logo *is* a dark navy-black canvas with a periwinkle wire. Physical scene: a developer at a desk, editor in dark mode, glancing at this between problems. A light site would clash with the tool it's selling and read as a marketing site, not a dev tool. Dark, precise, calm.

Strategy: **Restrained → Committed.** Near-monochrome dark surface; the periwinkle accent carries identity at ~10–15% of the surface (the wire, the one CTA, links, the live motif). Not drenched.

## Color (OKLCH — tune to logo)

```css
:root {
  /* surfaces — deep navy-black, slight blue tint (the logo canvas) */
  --bg:          oklch(0.16 0.020 264);
  --surface:     oklch(0.205 0.022 264);
  --surface-2:   oklch(0.255 0.024 264);
  --border:      oklch(0.30 0.020 264);   /* hairline */
  --border-strong: oklch(0.40 0.020 264);

  /* ink */
  --ink:         oklch(0.965 0.008 264);  /* off-white primary — ~14:1 on bg */
  --muted:       oklch(0.745 0.018 264);  /* secondary — ≥4.6:1 on bg, body-safe */
  --faint:       oklch(0.620 0.018 264);  /* labels/meta only, never body copy */

  /* accent — the "wire" periwinkle */
  --accent:      oklch(0.715 0.140 274);
  --accent-hover:oklch(0.775 0.150 274);
  --accent-ink:  oklch(0.17 0.030 274);   /* dark text ON the periwinkle button */
  --accent-soft: oklch(0.715 0.140 274 / 0.12); /* tint washes, focus halos */

  /* semantic — used sparingly; mirror the app's quality colors */
  --ok:   oklch(0.78 0.150 150);
  --warn: oklch(0.82 0.140 80);
  --bad:  oklch(0.68 0.190 25);
}
```

Contrast rules: `--ink` and `--muted` on `--bg`/`--surface` only for text. The periwinkle is a *light* accent → button text is `--accent-ink` (dark), link text is `--accent` on dark bg (verify ≥4.5:1; bump L if close). Never gray-on-color; never `--faint` for body.

## Typography

Contrast-axis pairing (geometric grotesque + mono) — echoes the rounded wordmark and the dev/CLI nature.

- **Display + UI + body:** **Geist** (Vercel's grotesque; self-host woff2). Nods to the wordmark's rounded geometry and the engineered-restraint reference. Variable weights 400/500/600/700.
- **Mono:** **Geist Mono** for the invite link, version string, terminal/menu paths, and any code. This is where dev-native character lives.

Scale & rules:
- Hero/display: `clamp(2.5rem, 6vw, 5rem)` — ceiling ≤ 6rem. Weight 600. `letter-spacing: -0.03em` (floor −0.04em). `text-wrap: balance`.
- Section headings: `clamp(1.5rem, 3vw, 2.25rem)`, weight 600, `text-wrap: balance`.
- Body: 16–18px, weight 400, line-height 1.6, `max-width: 68ch`, `text-wrap: pretty`.
- Mono (invite link / paths): 14–15px, weight 500, slight `--surface-2` chip background, rounded.
- No tiny tracked uppercase eyebrows (banned). Labels, when needed, are sentence-case `--faint` at normal tracking.

## Motion

Intentional, restrained, exponential ease-out. The hero motif is the signature; everything else is quiet.

- **Hero connect motif (signature):** the logo's two screen-glyphs with the periwinkle wire *drawing* between them (SVG `stroke-dashoffset` 0→full, ~900ms `ease-out-expo`), settling into the looped connection, with a soft accent glow pulse on "connect." This visualizes P2P pairing. Optionally a slow idle shimmer along the wire. **Reduced-motion:** render the fully-connected static state, no draw.
- **Reveal:** scroll-triggered fade+rise (translateY 12px→0, opacity), staggered within the 3-step flow only. Content is visible by default (CSS `@starting-style` or IntersectionObserver that *enhances*, never gates). Each reveal fits its element; no uniform whole-page reflex.
- **Interaction:** Download button — subtle lift (translateY −2px) + `--accent-soft` glow on hover, 160ms ease-out. Links — accent underline grow. Collapsibles (permissions help) — height/opacity ease-out.
- Durations 140–600ms. Curves: `cubic-bezier(0.16,1,0.3,1)` (expo) / quint. No bounce, no elastic.
- Tooling: vanilla CSS + a few lines of JS (IntersectionObserver, the SVG draw). The follow-up `web-animation-design` pass may elevate the hero motif (GSAP/anime/motion) — keep it tasteful and reduced-motion-safe. Every animation needs the reduce fallback.

## Components

No card grids, no testimonials, no pricing, no "trusted by." Typographic and demonstrative.

- **Nav:** mark+wordmark left; `Download` (accent) + `GitHub` (ghost) right. Transparent → hairline `--border` bottom on scroll.
- **Hero:** wordmark/tagline ("Screen-share, talk, and draw on each other's screens. Peer-to-peer, no account."), the **connect motif**, primary **Download for macOS** CTA, and a quiet meta line: `macOS 12.3+ · unsigned beta · no sign-up`.
- **Experiment banner:** one candid sentence high on the page, in `--surface` with a hairline, accent dot. "This is an experiment. Rough, unsigned, may break. If it's useful, I'll give it more time." + feedback link.
- **How it works:** 3 plain steps laid along a faint wire line (echo the motif), not icon cards. Real language (Host → send link → they see your screen + draw).
- **First-launch help:** collapsible "Opening it the first time" (Gatekeeper "Open Anyway" path in mono) + "Granting permissions" (Screen Recording + Microphone). Real text, screen-reader friendly.
- **Footer:** feedback (GitHub issues / mailto), honest fine print (P2P, end-to-end encrypted, screen/audio never stored), version.

## Layout

- Single column, content `max-width` ~ 760px (hero/motif may bleed wider to ~1040px). Centered, generous vertical rhythm, lots of negative space (restraint).
- Spacing scale: 4 / 8 / 12 / 16 / 24 / 40 / 64 / 96 px — vary for rhythm, don't uniformly pad.
- Flexbox for 1D rows; Grid only where genuinely 2D. Semantic z-index scale.
- Responsive: fluid clamps; **test hero copy at 360 / 768 / 1280** — no headline overflow. Viewport is part of the design.

## Imagery

The single illustrative element is the brand motif (two screens + wire), used live in the hero and as a faint structural line in "how it works." Use `logo.png` for the nav mark + favicon (and as the source for the app `.icns`). No stock imagery, no generic icon sets, no 3D.
