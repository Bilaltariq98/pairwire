# pairwire

P2P screen sharing and pair programming over QUIC. No server required — peers connect directly via [iroh](https://github.com/n0-computer/iroh) with NAT traversal and relay fallback.

## Status

Phase 3 complete. Screen sharing, H.264 encode/decode, audio pipeline, session reconnection, and macOS menu bar are all working. Terminal sharing (Phase 4) is in progress.

## Features

- **P2P connectivity** — direct connection via iroh QUIC; relay fallback when NAT traversal fails
- **Screen sharing** — H.264 encode/decode via FFmpeg (hardware-accelerated on Apple Silicon)
- **Audio** — Opus encode/decode via cpal; voice activity detection skips silence
- **Session reconnection** — automatic exponential-backoff retry with UI overlay
- **Remote cursor** — host cursor position shown as overlay on guest viewer
- **macOS menu bar** — Copy Invite Link, Disconnect, Quit (optional feature)

## Requirements

- Rust stable (1.80+)
- macOS: `brew install ffmpeg pkg-config`
- Linux: `apt install libavcodec-dev libavformat-dev libavutil-dev libswscale-dev pkg-config libasound2-dev libpipewire-0.3-dev`
- Windows: vcpkg FFmpeg or pre-built binaries

### macOS permissions

Grant these before running with real capture/audio:

- **System Settings → Privacy & Security → Screen Recording** — required for `--features capture`
- **System Settings → Privacy & Security → Microphone** — required for `--features audio`

## Building

```bash
# Default (no FFmpeg, no capture, no GUI — useful for CI/tests)
cargo build

# With screen sharing + audio (typical development build)
cargo build --features encode,capture,audio

# With GUI viewer
cargo build --features encode,capture,audio,gui

# Release (required for usable performance)
cargo build --release --features encode,capture,audio,gui
```

## Running

### Host (share your screen)

```bash
cargo run --release --features encode,capture,audio -- host
```

Prints an invite link:

```
  Invite link:

    pairwire://abc123...?relay=https://...&token=...
```

Options:
```
--fps N       Target capture frame rate (default: 30)
--width W     Capture width in pixels (default: 1920)
--height H    Capture height in pixels (default: 1080)
```

### Guest (view remote screen)

```bash
cargo run --release --features encode,audio,gui -- join '<invite-url>'
```

Options:
```
--no-gui      Headless mode — connect without opening a viewer window
```

The guest does not need `--features capture` (they receive, not capture).

## Feature flags

| Flag | Enables | Requires |
|---|---|---|
| `encode` | H.264 encode + decode via FFmpeg | System FFmpeg libraries |
| `capture` | Real screen capture via scap | Screen Recording permission |
| `audio` | Mic capture + speaker playback via cpal | Microphone permission |
| `gui` | egui viewer window | Display + GPU |
| `tray` | macOS menu bar icon | macOS only; GTK on Linux |

Features are additive — combine as needed. Without `encode`, the handshake completes but no video is sent. Without `capture`, the host sends a synthetic blue test pattern instead of your real screen.

## Development

```bash
just check    # cargo check --workspace
just lint     # cargo clippy --workspace -- -D warnings
just test     # cargo test --workspace
just fmt      # cargo fmt --all
just all      # all of the above
```

## Testing and packaging

See [`docs/manual-test-plan.md`](docs/manual-test-plan.md) for:
- macOS Screen Recording and Microphone permission setup
- Step-by-step test matrix (T1–T8) covering headless, video, audio, reconnection, and tray
- Log reference — what each log line means
- Release binary packaging for macOS (universal), Linux, and Windows
- `.app` bundle creation for macOS permission management

## Known gaps (next up)

- **Real mic capture** — audio pipeline is wired but host calls the fake 440Hz sine source; real cpal capture not yet connected in `session.rs`
- **Remote control** — input injection crate exists (macOS CGEvent) but not wired into the session
- **Clipboard sync** — implemented in `crates/input`, not wired into session
- **Terminal sharing** — Phase 4; PTY forwarding over QUIC (the differentiating feature)

## Architecture

```
crates/
├── core/        Shared types, config, error enum
├── protocol/    Wire protocol (postcard serialization)
├── transport/   iroh QUIC + moq-lite media tracks
├── capture/     Screen capture (scap) + H.264 encode/decode (FFmpeg)
├── audio/       Mic capture + Opus encode/decode + speaker playback (cpal)
├── input/       Keyboard/mouse injection + clipboard sync
├── terminal/    PTY sharing (Phase 4)
└── app/         Binary entry point + egui viewer + session orchestration
```

**Thread model:** single process, multi-threaded. Capture, encode, decode, and audio each run on dedicated OS threads. tokio runtime handles all networking. UI runs on the main thread.

**Transport:** iroh QUIC for connectivity. moq-lite tracks for screen (priority 3) and audio (priority 0). Raw QUIC streams for control and input. QUIC datagrams for cursor position (60Hz, unreliable).

## Protocol

| Data | Transport | Priority |
|---|---|---|
| Audio | moq-lite track | 0 (highest) |
| Control / input | Reliable QUIC stream | 1 |
| Cursor position | QUIC datagram | unreliable |
| Screen frames | moq-lite track | 3 (lowest) |

Invite links encode the iroh `EndpointAddr` (public key + relay URL) and a session token. No central server is involved in the connection after the initial relay-assisted hole punch.
