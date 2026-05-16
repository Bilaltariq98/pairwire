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

## T9: Settings (config.toml three-layer merge)

Tests: P4.7 — `config.toml` is created on first run, settings flow into runtime behavior, and CLI flags override the file.

**Config path** (macOS): `~/Library/Application Support/pairwire/config.toml`. On Linux: `~/.config/pairwire/config.toml`. The exact resolved path is logged at startup as `loaded settings path=...`.

```bash
# 0. Optional: clean slate so first-run behavior is observable.
rm -f "$HOME/Library/Application Support/pairwire/config.toml"

# 1. First run — should create the file with safe defaults.
cargo run --release --features encode,capture -- host
# (Ctrl+C after you see "Invite link" and "loaded settings path=...".)
```

**Expected after step 1:**
- [ ] Startup log includes `loaded settings path=<...>/pairwire/config.toml`
- [ ] That file now exists
- [ ] First line of the file is `# pairwire config — edit freely. ...`
- [ ] File contains `[identity]`, `[capture]`, `[audio]`, `[permissions]`, `[terminal]`, `[tunnels]`, `[ui]` sections
- [ ] `permissions.remote_control = "denied"` (safe default)
- [ ] `tunnels.blocked_ports = [3306, 5432, 6379, 27017]` (database ports pre-blocked)

```bash
# 2. Settings → runtime: change auto-grant in the file, then start.
#    Replace the line in config.toml:
#      remote_control = "denied"   →   remote_control = "auto_grant"
cargo run --release --features encode,capture,gui -- host
```

**Expected after step 2:**
- [ ] No `--allow-remote-control` flag passed, yet the host auto-grants on the first `ControlRequest` from the guest (same behavior as passing the flag). Verify by connecting a guest and pressing `Cmd+Shift+R` — control should grant without a tray click.

```bash
# 3. CLI overrides file: keep auto_grant in the file, but force OFF on the CLI.
#    Currently there is no --deny-remote-control flag; the inverse is to set
#    the file back to "denied" and add --allow-remote-control:
#      remote_control = "denied"
cargo run --release --features encode,capture,gui -- host --allow-remote-control
```

**Expected after step 3:**
- [ ] Despite `denied` in the file, the CLI flag wins — first guest `Cmd+Shift+R` is auto-granted.

```bash
# 4. Setting flows: fps from file.
#    In config.toml under [capture]:  max_fps = 24
cargo run --release --features encode,capture -- host
```

**Expected after step 4:**
- [ ] Startup log shows `display resolution detected` followed by frame rate ~24 fps in the encode thread (run `RUST_LOG=pairwire=debug` to see the encoder config).
- [ ] CLI `--fps 60` overrides this — verify by appending `--fps 60` and re-running.

```bash
# 5. Corrupted file fallback.
echo 'this is = = not toml [[[' > "$HOME/Library/Application Support/pairwire/config.toml"
cargo run --release --features encode,capture -- host
```

**Expected after step 5:**
- [ ] Startup log includes `WARN ... failed to load config; using defaults path=...`
- [ ] Pairwire still starts and prints an invite link — the bad config does **not** abort startup.

**Cleanup:** Restore the file by removing it (first-run regenerates) or `cargo run -- host` once with no edits and let it rewrite on save in a future step.

---

## T10: `pairwire config` subcommands

Tests: P4.7 step 4 — `path`, `get`, `set`, `edit` operate on the same `config.toml` that `host`/`join` consume.

```bash
# Start from a clean slate.
rm -f "$HOME/Library/Application Support/pairwire/config.toml"
```

### 1. `path` prints the resolved location

```bash
cargo run -- config path
```

**Expected:**
- [ ] Prints something ending in `pairwire/config.toml` (macOS: `~/Library/Application Support/pairwire/config.toml`)
- [ ] Exit code 0; no extra output

### 2. `get` reads from the file (and creates it on first run)

```bash
cargo run -- config get capture.max_fps
```

**Expected:**
- [ ] Prints `30` (the default).
- [ ] The config file now exists (first-run template was written by `get`).
- [ ] `cargo run -- config get permissions.remote_control` prints `denied` (no quotes).
- [ ] `cargo run -- config get capture.nope` exits non-zero with a clear "key not found" message.

### 3. `set` writes a value and preserves comments

```bash
cargo run -- config set capture.max_fps 60
cargo run -- config get capture.max_fps
```

**Expected:**
- [ ] Second `get` prints `60`.
- [ ] First line of `config.toml` is still `# pairwire config — edit freely…` — the comment block from step 2 must survive the write.
- [ ] `cargo run -- config set permissions.remote_control '"auto_grant"'` then `get` prints `auto_grant`.

### 4. `set` validates and refuses bad input

```bash
cp "$HOME/Library/Application Support/pairwire/config.toml" /tmp/pw.before
cargo run -- config set permissions.remote_control '"nonsense"'   # should fail
diff "$HOME/Library/Application Support/pairwire/config.toml" /tmp/pw.before
```

**Expected:**
- [ ] The `set` command exits non-zero and prints an error referencing the bad value or the key.
- [ ] `diff` shows no changes — the file is **not** corrupted by a rejected write.

### 5. End-to-end with `host`

After `set`-ing `capture.max_fps = 45` and `permissions.remote_control = "auto_grant"`:

```bash
cargo run --features encode,capture,gui -- host
```

**Expected:**
- [ ] Startup log shows `loaded settings path=<...>/config.toml`.
- [ ] Guest `Cmd+Shift+R` auto-grants without `--allow-remote-control` (proves `set` value flowed through).
- [ ] Encode thread runs at ~45 fps (visible with `RUST_LOG=pairwire=debug`).

### 6. `edit` opens `$EDITOR`

```bash
EDITOR=cat cargo run -- config edit
```

**Expected:**
- [ ] `cat` prints the current `config.toml` contents and exits.
- [ ] Without `EDITOR` set, `vi` is launched (smoke-test this manually).

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
