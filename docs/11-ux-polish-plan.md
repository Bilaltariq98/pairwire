# UX Polish Plan — Session Surface & Interaction Layer

> This document covers the work needed to make pairwire feel like a real tool
> rather than a demo. It follows P3.5.3 (drawing) and precedes Phase 4 (terminal/tunneling).
> Each item has a "why" — the user problem it solves — before the "what".

---

## Context: What We Have vs What Users Experience

Right now a session looks like this:

- **Host**: runs `pairwire host --allow-remote-control`, sees a tray icon, no feedback
- **Guest**: sees a black egui window with a crosshair cursor overlay and an FPS counter

The core pipeline works. But there's no way to mute, no way to know who has control,
no way to adjust quality, and the cursor looks like a debug artifact. This document
defines the work to close that gap.

---

## 1. Mic Mute Toggle

**Why**: Audio is always-on right now. In any real pairing session you need to mute —
background noise, a phone call, a cough. Without mute, people will disable audio
entirely (`--no-audio`), which is worse. This is table-stakes for any communication tool.

**What's needed**:

`AudioCaptureThread` already has a `stop_flag: Arc<AtomicBool>`. We need a second
flag: `muted: Arc<AtomicBool>`. When muted, the capture loop sends silence frames
(already have `OpusPacket::silence()`) instead of real audio. This keeps the stream
alive (no decoder discontinuity) while suppressing the mic.

Expose via:
- A `mute()` / `unmute()` method on `AudioCaptureThread`
- A `watch::Sender<bool>` in session so the UI can toggle it
- A keyboard shortcut in the viewer (`M` key, or `Cmd+Shift+M`)
- A visual indicator in the viewer: mic icon, red when muted

**Scope**: ~50 LoC in `capture.rs` + ~30 LoC in `viewer.rs` + session wiring.

---

## 2. Cursor Presence — Tuple-Style

**Why**: The current crosshair is a red circle + lines at a fixed size. It looks like
a debug overlay. Tuple's cursor is a small arrow with the peer's name tag below it —
immediately readable, unobtrusive, clearly "this is a person". The cursor is the
primary way you communicate "look here" during a session. It needs to feel intentional.

**Current state**:
- Host cursor: already in the captured video (scap captures the system cursor)
- Guest cursor: red crosshair overlay in `viewer.rs` lines 122–150
- `CursorUpdate.peer_id` exists but is always 0 — not used for color selection

**What's needed**:

1. **Replace the crosshair with an arrow cursor + name tag**
   - Small filled arrow (egui `painter.add(Shape::convex_polygon(...))`)
   - Peer name label below the arrow tip (from `peer_id` → display name mapping)
   - Color derived from `peer_id` (hash → hue, consistent across session)

2. **Hide the system cursor in the captured video** (or accept it — see note below)
   - scap captures the macOS system cursor baked into the frame
   - This means the host cursor appears twice: once in the video, once as an overlay
   - Short-term: accept it. The host cursor in video is accurate; the overlay is for
     the guest's own cursor when they're in the viewer
   - Long-term: scap has a `exclude_cursor` option — use it and render both cursors
     as overlays for consistency

3. **Guest cursor sent to host**
   - Currently `CursorSender` on the host sends host position to guest
   - Guest viewer should also send its cursor position back to host
   - Host needs a way to display it (session bar or a small overlay on their desktop)
   - Deferred: host-side cursor display requires a host viewer or HUD

**Scope**: ~80 LoC in `viewer.rs` (replace crosshair rendering). Name tag requires
a `peer_name` field in `CursorUpdate` or a side-channel name map.

---

## 3. Resolution Detection & Auto-Config

**Why**: Right now `HostArgs` defaults to `1920×1080` hardcoded. On a 14" MacBook Pro
the actual display is `3024×1964` (Retina). scap captures at native resolution, then
the encoder scales down to 1920×1080. This means:
- We're doing an unnecessary scale on every frame (CPU cost)
- The user has to know to pass `--width 3024 --height 1964` to avoid it
- On a 27" 5K iMac the defaults are even more wrong

The right behaviour: detect the actual display resolution at startup and use it as
the encode target (or a sensible fraction of it for bandwidth reasons).

**What's needed**:

1. **Detect primary display resolution at startup**
   - macOS: `CGDisplay::main().bounds()` — already used in `cursor_macos.rs`
   - Return `(logical_width, logical_height, scale_factor)`
   - Retina: logical=1512×982, scale=2.0, physical=3024×1964

2. **Resolution presets** (not arbitrary dimensions)
   ```
   Native      — capture at physical resolution, no scale
   1080p       — cap at 1920×1080 (default for bandwidth)
   720p        — cap at 1280×720 (low bandwidth / slow connection)
   ```
   The encode resolution should be `min(native, preset)`.

3. **Wire into `HostArgs` / `CaptureConfig`**
   - `--resolution native|1080p|720p` flag (replaces `--width`/`--height`)
   - Default: `1080p` (safe for most connections)
   - `native` for local LAN sessions where bandwidth isn't a concern

4. **Communicate to guest in handshake**
   - `Handshake.display_width/height` already exists — populate it from detected
     resolution, not from `CaptureConfig` (which may be downscaled)
   - Guest viewer uses this to set initial window aspect ratio

**Scope**: ~60 LoC in `capture_thread.rs` + `cli.rs` + `session.rs`.

---

## 4. What to Share (Deferred — Design Notes)

**Why**: Right now pairwire always shares the primary display. In practice:
- Developers often have 2 monitors — they want to share only one
- They might want to share a specific window (browser, IDE) not the whole screen
- scap supports window-level capture already

**Why deferred**: This requires a picker UI (list of displays/windows with previews),
which is a meaningful chunk of work. The value is real but it's not blocking anything.
The right time to build this is alongside the session bar (Phase 4.7 / config system).

**Design note for when we get there**:
- `pairwire host --window "Xcode"` — share by window name (CLI-first)
- `pairwire host --display 2` — share secondary display
- GUI picker: thumbnail grid of available sources, shown before session starts
- scap's `Target` enum already supports `Display(id)` and `Window(id)`

---

## 5. In-Viewer Session Controls (HUD)

**Why**: The viewer currently has an FPS counter in the corner and nothing else.
During a session you need to know: who has control, connection quality, whether
your mic is muted. Without this, users have no feedback and no way to act.

This is the "session bar" from `docs/07-ux-design.md` but scoped to what's
achievable now without a full toolbar widget.

**Minimal HUD** (bottom-right corner of viewer, stacked badges):

```
┌─────────────────┐
│  119 fps  🟢    │   ← FPS + connection quality dot
│  🎤 muted       │   ← mic state (only shown when muted)
│  You have ctrl  │   ← control state (only shown when active)
└─────────────────┘
```

- Connection quality: green/yellow/red dot from `QualityMonitor` output
- Mic state: driven by the mute watch channel from item 1
- Control state: "You have control" / "Host has control" — needs a `ControlState`
  watch channel from session to viewer

**Scope**: ~60 LoC in `viewer.rs`. Requires the mute channel (item 1) and a
`ControlState` watch channel wired from session.

---

## Recommended Build Order for Next Session

These are ordered by impact-per-hour, not by dependency:

| Priority | Task | Why now | Effort |
|---|---|---|---|
| 1 | **P3.5.3 Drawing** | Completes Phase 3.5. Unblocks the "pause and plan" milestone. | Medium |
| 2 | **Resolution auto-detect** | Fixes a real bug (wrong defaults on Retina). Low risk, high payoff. | Small |
| 3 | **Mic mute** | Table-stakes for any real session. Simple flag in capture thread. | Small |
| 4 | **Cursor polish** (arrow + name tag) | Makes the tool feel intentional vs debug. | Small–Medium |
| 5 | **Minimal HUD** (control state + quality dot) | Closes the "no feedback" gap. | Small |

Items 2–5 are all viewer/session surface work that can be done in one focused session.
They don't require new protocol types or transport changes — just wiring existing
data into the UI.

---

## What This Unlocks

After this work:
- A session feels like a tool, not a prototype
- Mic mute means people will actually use audio
- Cursor polish means "look here" communication works naturally
- Resolution auto-detect means it works correctly on the machines people actually have
- The HUD means users know what state they're in

This is the foundation for showing pairwire to anyone outside the team.
