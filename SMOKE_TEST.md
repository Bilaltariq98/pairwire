# Smoke Test Checklist

Manual verification for pairwire features. Run these on real hardware before cutting a release or after major changes.

## Prerequisites

- Two machines (or one machine with two terminals for basic tests)
- macOS with Screen Recording + Microphone permissions granted
- FFmpeg installed (`brew install ffmpeg`)
- Headphones recommended for audio tests (no echo cancellation)

### Environment setup (macOS)

```bash
export PKG_CONFIG_PATH="$(brew --prefix ffmpeg)/lib/pkgconfig:$(brew --prefix)/lib/pkgconfig"
export BINDGEN_EXTRA_CLANG_ARGS="-I$(brew --prefix ffmpeg)/include"
```

### Build

```bash
cargo build --release --features encode,capture,audio,gui
```

---

## T1: Screen Sharing (same machine)

Tests: capture, encode, moq transport, decode, viewer rendering.

```bash
# Terminal 1
cargo run --release --features encode,capture,gui -- host

# Terminal 2 — copy the invite link from terminal 1
cargo run --release --features encode,gui -- join '<invite-link>'
```

**Expected:**
- [ ] Guest viewer opens within 3 seconds
- [ ] Host screen is visible in the guest viewer
- [ ] FPS counter shows > 0 in top-right corner
- [ ] Host cursor (red crosshair) moves when you move the mouse on the host
- [ ] Resizing the viewer window re-scales the image without distortion
- [ ] Ctrl+C on either terminal cleanly shuts down without panics

**If no video appears:** check that `encode` and `capture` features are enabled on the host. Look for "encode thread started" and "capture thread started" in host logs.

---

## T2: Remote Control (two machines)

Tests: input forwarding, CGEvent injection, permission gating.

**Machine A (host):**
```bash
cargo run --release --features encode,capture,gui -- host --allow-remote-control
```

**Machine B (guest):**
```bash
cargo run --release --features encode,gui -- join '<invite-link>'
```

**Expected:**
- [ ] Guest clicks in viewer window — click lands at the correct position on host
- [ ] Guest types — keystrokes appear on the host (open a text editor on host to verify)
- [ ] Guest scrolls in viewer — host scrolls at the cursor position
- [ ] Modifier keys work (Shift, Ctrl/Cmd, Alt)

**Permission test — without the flag:**
```bash
# Host without --allow-remote-control
cargo run --release --features encode,capture,gui -- host
```
- [ ] Guest typing does NOT appear on host (events silently dropped)
- [ ] Host log shows "permission: Denied"

**macOS Accessibility:** if input injection doesn't work even with `--allow-remote-control`, grant Accessibility permission to the terminal app in System Settings > Privacy & Security > Accessibility.

---

## T3: Bidirectional Audio (two machines, headphones required)

Tests: Opus encode/decode, moq audio track, datagram audio, cpal capture/playback.

**Machine A (host):**
```bash
cargo run --release --features encode,capture,audio -- host
```

**Machine B (guest):**
```bash
cargo run --release --features encode,audio,gui -- join '<invite-link>'
```

**Expected:**
- [ ] Host speaks into mic → guest hears through speakers/headphones
- [ ] Guest speaks into mic → host hears through speakers/headphones
- [ ] Both directions work simultaneously
- [ ] No audio when nobody is speaking (VAD silence detection works)
- [ ] Audio quality is intelligible for voice

**Without `audio` feature:** both sides use a fake 440Hz sine wave. Useful for testing the pipeline without a mic.

**Feedback warning:** on the same machine without headphones, mic picks up speaker output → feedback loop. Always use headphones or test on separate machines.

---

## T4: Quality Adaptation (simulated)

Tests: QualityMonitor tier changes, encoder fps adaptation.

This is hard to test without network simulation. Check the logs instead:

```bash
RUST_LOG=info cargo run --release --features encode,capture -- host
```

**Expected in logs:**
- [ ] "quality monitor started" appears after handshake
- [ ] Periodic "connection health sample" at debug level (set `RUST_LOG=debug`)
- [ ] If you degrade the network (e.g., `sudo tc qdisc add` on Linux), see "quality tier changed" with reduced fps

---

## T5: Session Reconnection

Tests: reconnect logic, connection state overlay.

```bash
# Start host and guest normally
# Then kill the host (Ctrl+C)
# Watch the guest viewer
```

**Expected:**
- [ ] Guest viewer shows "Reconnecting... (1/3)" overlay
- [ ] After ~7 seconds (1s + 2s + 4s backoff), shows "Connection lost"
- [ ] No panics or crashes on either side

---

## T6: Session Authentication

Tests: session token verification in handshake.

```bash
# Start host
cargo run --release --features encode,capture -- host

# Tamper with the invite link — change a character in the token
cargo run --release --features encode,gui -- join 'pairwire://...?token=0000000000000000000000000000000'
```

**Expected:**
- [ ] Guest fails to connect with a handshake error (token mismatch)
- [ ] Host log shows "session token mismatch — rejecting peer"

---

## T7: Config Validation

Tests: invalid config rejection at startup.

```bash
# Odd dimensions (H.264 requires even)
cargo run --release --features encode,capture -- host --width 1921

# Zero fps
cargo run --release --features encode,capture -- host --fps 0
```

**Expected:**
- [ ] Both commands fail immediately with a clear error message
- [ ] No panic, no partial startup

---

## T8: Headless Mode

Tests: guest without GUI.

```bash
cargo run --release --features encode,capture,audio -- host

cargo run --release --features encode,audio -- join '<invite-link>' --no-gui
```

**Expected:**
- [ ] Guest connects and receives frames (check logs for "video track" messages)
- [ ] Audio still works in headless mode
- [ ] No GUI window opens on the guest

---

## T9: Audio Mute Controls

Tests: P4.8 — `--mute` CLI flag and runtime mute toggles on both host (tray) and guest (viewer).

### 1. `--mute` starts the host muted

```bash
# Host: start with mic muted
cargo run --release --features encode,capture,audio --no-default-features --features tray -- host --mute

# Guest:
cargo run --release --features encode,audio,gui -- join '<invite-link>'
```

**Expected:**
- [ ] Host startup log includes `host mic mute state changed muted=true` once the audio pipeline comes up
- [ ] Guest hears silence (no host voice) — speak on host to confirm
- [ ] Host tray menu shows "Unmute Microphone" (label reflects current state)

### 2. Toggle mute mid-session via host tray

With the session above still running:

- [ ] Click tray → "Unmute Microphone" → log shows `tray: mute toggled muted=false`
- [ ] Guest now hears host voice
- [ ] Tray label flips to "Mute Microphone"
- [ ] Click again → guest goes silent again within ~100ms

### 3. `--mute` starts the guest muted

```bash
cargo run --release --features encode,capture,audio -- host
cargo run --release --features encode,audio,gui -- join '<invite-link>' --mute
```

**Expected:**
- [ ] Guest viewer HUD shows "🔇 Muted" badge in the bottom-right
- [ ] Guest toolbar button reads "🔇 Unmute"
- [ ] Host hears silence — guest speaks to confirm
- [ ] Click guest toolbar "🔇 Unmute" (or press Cmd/Ctrl+Shift+M) → host hears voice; badge disappears

### 4. End-to-end: both sides toggling mute independently

Run the same setup (no `--mute` flags). On both sides:

- [ ] Mute the guest via the viewer toolbar → host stops hearing guest (host still hears self if monitor enabled — that's expected)
- [ ] Mute the host via tray → guest stops hearing host
- [ ] Unmute either side → audio resumes on that direction without restarting the session

### 5. Initial state survives short audio-pipeline restarts

`--mute` is applied each time `start_host_audio` constructs a new capture handle (so it survives guest reconnects). Smoke-test this by:

```bash
cargo run --features encode,capture,audio --no-default-features --features tray -- host --mute
# Connect a guest, then kill the guest, then reconnect a guest.
```

**Expected:**
- [ ] On the second guest connection, the host mic is still muted (no need to re-set anything).
- [ ] Host log shows `host mic mute state changed muted=true` once on each capture restart.

---

## Log Reference

Key log lines and what they mean:

| Log | Meaning |
|-----|---------|
| `iroh endpoint bound` | Local QUIC endpoint ready |
| `invite link generated` | Host has a relay URL and is ready for connections |
| `guest connected` / `connected to host` | QUIC connection established |
| `handshake complete` | Protocol version + capabilities + session token verified |
| `input receiver started — permission: FullControl` | Remote control is active |
| `input receiver started — permission: Denied` | Remote control blocked (no `--allow-remote-control`) |
| `quality monitor started` | Connection health polling active |
| `quality tier changed` | Encoder adapting to network conditions |
| `encode thread started` | FFmpeg encoder ready |
| `capture thread started` | scap screen capture active |
| `audio capture thread started` | Mic capture active |
| `guest audio pipeline started` | Host is decoding + playing guest audio |
| `host disconnected` / `guest disconnected` | Clean session end |

---

## UX Polish Epic — Manual Verification

Run host (without auto-grant):

```bash
cargo run --features encode,capture,audio,gui,tray -- host
```

Run guest (separate machine or process):

```bash
cargo run --features encode,capture,audio,gui,tray -- join "<invite>"
```

**Note:** Guest cursor position is logged at `debug!` level, not `info!`. Use the following env var when grepping for cursor events:

```bash
RUST_LOG=pairwire_app=debug cargo run --features encode,capture,audio,gui,tray -- host
```

Then grep for `guest cursor` in the host output.

**Verify:**

- [ ] Guest cursor logged on host (`RUST_LOG=pairwire_app=debug` shows `guest cursor peer_id=… x=… y=…`).
- [ ] Tray menu has `Allow Remote Control` (enabled) and `Revoke Control` (disabled, greyed out).
- [ ] Guest viewer HUD shows `View only` initially.
- [ ] `Cmd+Shift+R` on guest viewer → host log shows `guest requested control`; guest HUD switches to `Awaiting host approval…`. Press `Esc` to cancel a pending request.
- [ ] Click tray `Allow Remote Control` on host → guest HUD switches to `You have control`; guest can drive host mouse/keyboard.
- [ ] Captured stream shows no system cursor while control is granted (only the guest's own cursor overlay arrow is visible — host system cursor is hidden by rebuilding the scap capturer with `show_cursor=false`).
- [ ] Click tray `Revoke Control` on host → guest HUD returns to `View only`; host system cursor reappears in the captured stream.
- [ ] Run host with `--allow-remote-control` flag — first `Cmd+Shift+R` from the guest is auto-granted without a tray click.

**Expected tray state machine:**

| Control state | `Allow Remote Control` | `Revoke Control` |
|---|---|---|
| Idle (no request) | enabled | disabled |
| Pending (guest requested) | enabled | disabled |
| Granted | disabled | enabled |

**Known transition behaviour:** granting/revoking control triggers a scap capturer rebuild (~33–500ms depending on machine) which may cause a brief hitch in the video stream. This is expected.
