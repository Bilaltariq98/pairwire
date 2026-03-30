# CLAUDE.md — Claude Code / Droid Instructions

## Quick Context

Rust-native P2P pair programming tool. Terminal-first collaboration over QUIC (iroh), with screen sharing (scap + FFmpeg), audio (cpal + Opus), and remote control. See `docs/` for full research and specifications.

## Read Order for New Tasks

1. `PLAN.md` — find your task, understand what phase it belongs to
2. `AGENTS.md` — coding conventions, architecture, protocol reference
3. `CONVENTIONS.md` — **ground truth** for patterns established in this codebase
4. `LEARNINGS.md` — check for known gotchas before starting
5. Relevant `docs/` file(s) — deep context when needed:
   - Connectivity/NAT: `docs/01-iroh-noq-connectivity.md`
   - Media transport: `docs/02-moq-lite-vs-moq-transport.md`
   - QUIC primitives: `docs/03-quic-transport-primitives.md`
   - Capture/encode: `docs/04-screen-capture-encode-pipeline.md`
   - Architecture: `docs/05-concrete-architecture.md`
   - Competitive context: `docs/06-competitive-analysis.md`

## Validation Commands

```bash
just check    # cargo check --workspace
just lint     # cargo clippy --workspace -- -D warnings
just test     # cargo test --workspace
just fmt      # cargo fmt --all -- --check
just all      # all of the above in sequence
```

Run `just all` before considering any task complete.

## Critical Rules

- **Never `unwrap()` outside of tests.** Use `?` or explicit error handling with `thiserror`.
- **Never `println!` in library crates.** Use `tracing::{info, debug, warn, error}` with structured fields.
- **Never add a dependency without checking Cargo.toml first.** Prefer what's already in the workspace.
- **Always update LEARNINGS.md** when you discover a quirk, hit an unexpected issue, or find something that contradicts the docs.
- **Always reference the PLAN.md task ID** in commit messages (e.g., `P1.3: Implement screen capture thread`).
- **Wire protocol types go in `crates/protocol/`.** Don't define message types in other crates.
- **Shared types go in `crates/core/`.** Config, error enums, common structs.

## Architecture Constraints

- **Single process, multi-threaded.** Capture and encode run on dedicated OS threads, not tokio tasks. Use channels to bridge to the tokio runtime.
- **moq-lite for media tracks** (screen, audio). **Raw QUIC streams/datagrams** for control, input, cursor.
- **iroh is the only network layer.** Don't introduce raw Quinn or other QUIC implementations.
- **FFmpeg is required for video encode/decode.** No pure-Rust alternative for hardware acceleration.
- **macOS first.** Build and test on macOS before cross-platform work.

## Common Pitfalls

- iroh's `Endpoint` is a singleton — don't create multiple per process.
- scap's capture callback runs on its own thread — don't block it. Send frames to a channel immediately.
- FFmpeg encoder/decoder contexts are NOT Send. Keep them on their dedicated thread.
- moq-lite groups map 1:1 to QUIC streams. Each keyframe should start a new group.
- QUIC datagrams have a max size (~1200 bytes). Cursor updates must stay under this.
- `postcard` serialization is NOT self-describing. Both sides must agree on the exact type layout. Version your protocol types.

## When You're Stuck

1. Check `LEARNINGS.md` — someone may have hit this before.
2. Check the relevant `docs/` research file — the answer may be in the analysis.
3. Check upstream crate documentation and examples (iroh, moq-dev/moq, scap).
4. If you discover something new, **add it to LEARNINGS.md** before moving on.
