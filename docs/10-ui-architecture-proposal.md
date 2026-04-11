# UI Architecture Proposal (DRAFT)

> **Status**: Early thinking. Not committed to. Captured so we don't lose the analysis.
> **Date**: 2025-04-07

## Context

Pairwire is currently CLI-first with an egui viewer for screen sharing sessions. This works well for dogfooding and proving core capabilities (screen share, audio, cursor, control takeover). But the enterprise-tier features (people/contacts, rooms, invite flows, team management, settings) will need richer UI than egui can comfortably deliver.

## Two UI Surfaces

| Surface | Nature | Best fit |
|---|---|---|
| **Session viewer** — screen texture, cursor overlays, drawing, in-session toolbar | Real-time GPU rendering, low-latency input | egui (already working) |
| **Lobby/management** — contacts, rooms, invites, settings, team admin | Rich forms, lists, search, styled components | Web tech (Tauri webview) |

## Proposed Architecture: Tauri + egui Hybrid

```
┌─────────────────────────────────────────────────┐
│                   Tauri Shell                    │
│  (window management, system tray, menus, IPC)   │
├──────────────────┬──────────────────────────────┤
│   Webview Panel  │      egui/wgpu Panel         │
│                  │                               │
│  - People list   │  - Screen texture rendering   │
│  - Rooms         │  - Cursor overlays            │
│  - Call links    │  - Drawing/annotations        │
│  - Settings      │  - In-session toolbar         │
│  - Team admin    │    (mic/cam/share toggles)    │
│  - Invite flows  │  - Quality indicators         │
│                  │                               │
│  HTML/CSS/JS     │  Rust-native GPU rendering    │
├──────────────────┴──────────────────────────────┤
│              Rust Core (shared)                   │
│  transport / capture / audio / protocol / input  │
└─────────────────────────────────────────────────┘
```

## Crate Structure Evolution

Current:
```
crates/app/  — monolith (CLI + egui viewer + session orchestration)
```

Future:
```
crates/
  ui-core/    — shared UI state, session state machine, command/event types
  viewer/     — egui viewer, toolbar overlays (extracted from app)
  app/        — Tauri shell (webview lobby + spawns viewer) OR CLI mode
```

Key principle: **`ui-core` owns the state** that both webview and egui viewer observe/mutate. Neither UI layer owns state directly.

## Reference: How RustDesk Does It

- Flutter UI + Rust core behind FFI bridge (`flutter_ffi.rs`)
- `Session<T: InvokeUiSession>` — generic over UI handler
- Message-passing (mpsc channels) between UI and io_loop
- Events streamed back via StreamSink (pub/sub)
- Flat toolbar composition — each control widget calls FFI directly

## Reference: Tuple's Lobby UI (Screenshots Analyzed)

Four tabs in a sidebar-nav layout:
1. **People** — contacts list with avatars, presence, favorites
2. **Rooms** — persistent virtual spaces (cards with dropdown menus)
3. **Call Link** — shareable invite URL, permissions (open/closed)
4. **Invite** — team invites (email + link), teammates vs outside collaborators

This is web-grade UI — not achievable in egui without significant pain.

## Current Strategy: CLI-First Dogfooding

For now, keep proving capabilities via CLI + egui viewer:
- Screen sharing works
- Audio bidirectional works
- Cursor rendering works
- Control takeover in progress
- System tray with invite link copy works

The Tauri/webview layer is additive — it wraps the existing viewer, it doesn't replace it. The CLI remains a first-class interface.

## Open Questions

- [ ] What does pairwire look like in its simplest free form?
- [ ] What interaction patterns define the paid/enterprise tier?
- [ ] Do we want the lobby as a persistent background app (like Tuple) or on-demand?
- [ ] Camera/video sharing — in the egui viewer as a PiP overlay? Separate panel?
- [ ] Should the in-session toolbar be egui or a floating webview overlay?
- [ ] Multi-party layout — gallery view? Who controls the grid?
