# Manual Test Plan

This document covers how to verify pairwire end-to-end, including OS permission setup, what to look for in logs, and how to package a release binary.

---

## 1. macOS Permission Setup

These must be done once before running with real capture or audio. The app will fall back to stubs if permissions are missing, so the binary won't crash — but you won't get real screen or audio.

### Screen Recording

1. Open **System Settings → Privacy & Security → Screen Recording**
2. Click **+** and add your terminal app (Terminal.app, iTerm2, Warp, etc.)
3. If the binary is already running, quit and relaunch — permissions are checked at capture start

Verify it worked: the host log should say:
```
INFO pairwire_capture::capture_thread: real capture thread started
```
Not:
```
WARN real screen capture unavailable — using fake source
```

### Microphone

1. Open **System Settings → Privacy & Security → Microphone**
2. Enable your terminal app

Verify: host log should say:
```
INFO pairwire_audio::capture: audio capture thread started
```

### First-run prompt

On first launch with `--features capture`, macOS will show a system dialog asking for Screen Recording permission. Click **Allow**, then relaunch — the permission takes effect on the next run.

---

## 2. Test Matrix

Run each test in order. Each builds on the previous.

### T1 — Baseline compile and unit tests

```bash
just all
```

Expected: all green, no warnings, 59+ tests pass.

---

### T2 — Headless connection (no GUI, no capture, no audio)

Two terminals on the same machine.

**Terminal 1:**
```bash
cargo run -- host
```

**Terminal 2:**
```bash
cargo run -- join '<invite-url>' --no-gui
```

What to verify:
- Host prints invite link within ~3 seconds of starting
- Guest connects: `INFO connected to host`
- Handshake completes: `INFO handshake complete capabilities=[Screen] video_codec=Some(H264)`
- Both sides log `encode feature not enabled` (expected without `--features encode`)
- Guest disconnects cleanly when you Ctrl-C the host

---

### T3 — Video pipeline (encode + decode, fake source)

**Terminal 1:**
```bash
cargo run --features encode -- host
```

**Terminal 2:**
```bash
cargo run --features encode,gui -- join '<invite-url>'
```

What to verify:
- Host log: `encode thread started width=1920 height=1080 fps=30`
- Guest log: `decode thread started`, then `decoder: first frame width=1920 height=1080`
- Viewer window opens showing a solid blue frame (fake source — this is correct)
- FPS counter in top-right corner of viewer shows ~9 fps in debug, ~30 fps in release
- Closing the viewer exits the guest cleanly

---

### T4 — Real screen capture

Requires Screen Recording permission (see section 1).

**Terminal 1:**
```bash
cargo run --release --features encode,capture -- host
```

What to verify:
- Host log: `real capture thread started` (not "fake source")
- Guest sees your actual screen in the viewer window
- Moving windows on the host is visible on the guest within ~100ms

If you see the blue fake source instead, check:
- Screen Recording permission is granted for your terminal
- You relaunched after granting permission

---

### T5 — Audio pipeline

**Terminal 1:**
```bash
cargo run --release --features encode,audio -- host
```

**Terminal 2:**
```bash
cargo run --release --features encode,audio,gui -- join '<invite-url>'
```

What to verify:
- Both sides log: `handshake complete capabilities=[Screen, Audio]`
- Host log: `audio pipeline started (host)`
- Guest log: `audio pipeline started (guest)`, `audio playback thread started`
- Guest can hear a 440Hz sine tone from the host (fake audio source — real mic not yet wired)

Note: `no Audio capability` in the log means the `audio` feature is missing on one side.

---

### T6 — Full stack (real screen + audio)

```bash
# Host
cargo run --release --features encode,capture,audio -- host

# Guest
cargo run --release --features encode,audio,gui -- join '<invite-url>'
```

What to verify:
- Real screen visible on guest
- Audio tone audible on guest
- FPS counter shows ~25-30 fps
- Cursor overlay (red crosshair) tracks the host mouse position
- Closing the viewer triggers clean shutdown on both sides

---

### T7 — Reconnection

```bash
# Host
cargo run --release --features encode,capture,audio -- host

# Guest
cargo run --release --features encode,audio,gui -- join '<invite-url>'
```

Once connected:
1. Kill the host with Ctrl-C
2. Watch the guest viewer — it should show "Reconnecting… (1/3)" overlay
3. After 3 attempts: "Connection lost" overlay
4. Restart the host with the same command (new invite link — guest will not auto-reconnect to a new link; this tests the overlay only)

What to verify:
- Reconnecting overlay appears within 1 second of host disconnect
- Attempts count up: 1/3, 2/3, 3/3
- "Connection lost" appears after all attempts fail
- Guest process exits cleanly after showing "Connection lost"

---

### T8 — macOS menu bar (tray feature)

```bash
cargo run --release --features encode,capture,audio,tray -- host
```

What to verify:
- A small icon appears in the macOS menu bar (top-right area)
- Clicking it shows: "Copy Invite Link", "Disconnect", "Quit pairwire"
- "Copy Invite Link" copies the invite URL to clipboard (paste to verify)
- "Disconnect" stops the session
- "Quit pairwire" exits the process

---

## 3. Log Reference

Key log lines and what they mean:

| Log line | Meaning |
|---|---|
| `iroh endpoint bound endpoint_id=...` | Transport layer ready |
| `invite link generated` | Ready to accept connections |
| `real capture thread started` | Screen Recording permission OK |
| `fake capture thread started` | No permission or `capture` feature off |
| `audio capture thread started` | Mic permission OK |
| `audio playback stub started` | `audio` feature off on this build |
| `handshake complete capabilities=[Screen, Audio]` | Both features negotiated |
| `handshake complete capabilities=[Screen]` | Audio missing on one side |
| `encode thread started` | FFmpeg encoder running |
| `decoder: first frame` | Video pipeline end-to-end working |
| `Reconnecting… (1/3)` | Connection dropped, retrying |
| `Connection lost` | All retries exhausted |

---

## 4. Packaging a Release Binary

### macOS (arm64 / Apple Silicon)

```bash
# Build
cargo build --release --features encode,capture,audio,gui,tray

# Binary location
ls -lh target/release/pairwire

# Optional: strip debug symbols (~30% smaller)
strip target/release/pairwire

# Copy to /usr/local/bin for system-wide use
sudo cp target/release/pairwire /usr/local/bin/pairwire
pairwire --help
```

### macOS universal binary (arm64 + x86_64)

```bash
# Add x86_64 target
rustup target add x86_64-apple-darwin

# Build both
cargo build --release --features encode,capture,audio,gui,tray --target aarch64-apple-darwin
cargo build --release --features encode,capture,audio,gui,tray --target x86_64-apple-darwin

# Combine
lipo -create \
  target/aarch64-apple-darwin/release/pairwire \
  target/x86_64-apple-darwin/release/pairwire \
  -output target/release/pairwire-universal

# Verify
lipo -info target/release/pairwire-universal
file target/release/pairwire-universal
```

### macOS .app bundle

A minimal app bundle lets macOS manage permissions properly (Screen Recording, Microphone) and allows drag-to-Applications install.

```bash
# Create bundle structure
mkdir -p pairwire.app/Contents/MacOS
mkdir -p pairwire.app/Contents/Resources

# Copy binary
cp target/release/pairwire pairwire.app/Contents/MacOS/pairwire

# Create Info.plist
cat > pairwire.app/Contents/Info.plist << 'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>pairwire</string>
  <key>CFBundleIdentifier</key>
  <string>com.pairwire.app</string>
  <key>CFBundleName</key>
  <string>pairwire</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>NSScreenCaptureUsageDescription</key>
  <string>pairwire needs screen recording to share your display.</string>
  <key>NSMicrophoneUsageDescription</key>
  <string>pairwire needs microphone access for audio sharing.</string>
  <key>LSUIElement</key>
  <true/>
</dict>
</plist>
EOF

# Run from bundle
open pairwire.app --args host
```

`LSUIElement = true` hides the app from the Dock (menu-bar-only app). Remove it if you want a Dock icon.

### Linux (static-ish binary)

```bash
# Install FFmpeg dev libs
sudo apt install libavcodec-dev libavformat-dev libavutil-dev \
  libswscale-dev libavdevice-dev pkg-config libasound2-dev

# Build (GUI requires display at runtime, not build time)
cargo build --release --features encode,capture,audio,gui

# Check dynamic dependencies
ldd target/release/pairwire

# Copy
sudo cp target/release/pairwire /usr/local/bin/pairwire
```

### Windows

```bash
# Install FFmpeg via vcpkg
vcpkg install ffmpeg:x64-windows

# Set env vars
set FFMPEG_DIR=C:\vcpkg\installed\x64-windows
set PKG_CONFIG_PATH=C:\vcpkg\installed\x64-windows\lib\pkgconfig

cargo build --release --features encode,capture,audio,gui
```

---

## 5. Environment Variables

| Variable | Effect |
|---|---|
| `RUST_LOG=pairwire=debug` | Verbose pairwire logs |
| `RUST_LOG=pairwire=debug,iroh=info` | pairwire debug + iroh info |
| `RUST_LOG=trace` | Everything (very noisy) |
| `FFMPEG_DIR` | Path to FFmpeg install (if not in pkg-config path) |
| `BINDGEN_EXTRA_CLANG_ARGS` | Extra clang args for ffmpeg-sys bindgen (macOS: `-I$(brew --prefix ffmpeg)/include`) |

Example with verbose logging:
```bash
RUST_LOG=pairwire=debug cargo run --release --features encode,capture,audio -- host
```
