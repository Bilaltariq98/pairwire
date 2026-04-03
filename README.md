# pairwire

P2P screen sharing and pair programming over QUIC. No server required — peers connect directly via [iroh](https://github.com/n0-computer/iroh) with NAT traversal and relay fallback.

## Status

Phase 3.5 in progress. Screen sharing, bidirectional audio, remote control input, quality adaptation, and session reconnection are all working. Drawing/annotations and terminal sharing are next.

## Features

- **P2P connectivity** — direct connection via iroh QUIC; relay fallback when NAT traversal fails
- **Screen sharing** — H.264 encode/decode via FFmpeg (hardware-accelerated on Apple Silicon)
- **Bidirectional audio** — Opus encode/decode; host mic to guest + guest mic to host; voice activity detection skips silence
- **Remote control** — guest keyboard/mouse forwarded to host via QUIC; host injects via CGEvent (macOS); permission-gated with `--allow-remote-control`
- **Quality adaptation** — monitors RTT/loss and dynamically reduces fps when network degrades; auto-recovers
- **Remote cursor** — host cursor position shown as overlay on guest viewer
- **Session reconnection** — automatic exponential-backoff retry with UI overlay
- **Session authentication** — invite link session token verified in handshake; rejects unauthorized peers
- **macOS menu bar** — Copy Invite Link, Disconnect, Quit (optional feature)

## Requirements

- Rust stable (1.80+)
- macOS: `brew install ffmpeg pkg-config`
- Linux: `apt install libavcodec-dev libavformat-dev libavutil-dev libswscale-dev pkg-config libasound2-dev libpipewire-0.3-dev`
- Windows: vcpkg FFmpeg or pre-built binaries

### macOS permissions

Grant these before running with real capture/audio:

- **System Settings > Privacy & Security > Screen Recording** — required for `--features capture`
- **System Settings > Privacy & Security > Microphone** — required for `--features audio`
- **System Settings > Privacy & Security > Accessibility** — required for `--allow-remote-control` (CGEvent injection)

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
--fps N                   Target capture frame rate (default: 30)
--width W                 Capture width in pixels (default: 1920)
--height H                Capture height in pixels (default: 1080)
--allow-remote-control    Auto-grant keyboard/mouse injection to guest
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

### Remote control

When the host starts with `--allow-remote-control`, the guest can click in the viewer window and type — keystrokes and mouse events are injected on the host via macOS CGEvent. Without the flag, all input events are silently dropped.

## Feature flags

| Flag | Enables | Requires |
|---|---|---|
| `encode` | H.264 encode + decode via FFmpeg | System FFmpeg libraries |
| `capture` | Real screen capture via scap | Screen Recording permission |
| `audio` | Mic capture + speaker playback via cpal | Microphone permission |
| `gui` | egui viewer window | Display + GPU |
| `tray` | macOS menu bar icon | macOS only; GTK on Linux |

Features are additive — combine as needed. Without `encode`, the handshake completes but no video is sent. Without `capture`, the host sends a synthetic blue test pattern instead of your real screen. Without `audio`, both sides use a fake 440Hz sine wave.

## Development

```bash
just check    # cargo check --workspace
just lint     # cargo clippy --workspace -- -D warnings
just test     # cargo test --workspace
just fmt      # cargo fmt --all
just all      # all of the above
```

See [SMOKE_TEST.md](SMOKE_TEST.md) for a manual test checklist covering screen sharing, remote control, audio, and quality adaptation.

## Architecture

```
crates/
├── core/        Shared types, config, error enum, diagnostics
├── protocol/    Wire protocol (postcard serialization) — cursor, input, drawing, reactions
├── transport/   iroh QUIC + moq-lite media tracks + datagram router + quality monitor
├── capture/     Screen capture (scap) + H.264 encode/decode (FFmpeg)
├── audio/       Mic capture + Opus encode/decode + speaker playback (cpal)
├── input/       Keyboard/mouse injection + clipboard sync + cursor polling
├── terminal/    PTY sharing (Phase 4)
└── app/         Binary entry point + egui viewer + session orchestration
```

**Thread model:** single process, multi-threaded. Capture, encode, decode, and audio each run on dedicated OS threads. tokio runtime handles all networking. UI runs on the main thread.

**Transport:** iroh QUIC for connectivity. moq-lite tracks for screen (priority 3) and host audio (priority 0). Bidi QUIC streams for control handshake and input forwarding. QUIC datagrams for cursor position (60Hz) and guest audio (Opus packets).

## Protocol

| Data | Transport | Priority |
|---|---|---|
| Host audio | moq-lite track | 0 (highest) |
| Guest audio | QUIC datagram (TAG_GUEST_AUDIO) | unreliable |
| Control / handshake | Reliable QUIC bidi stream | 1 |
| Input forwarding | Reliable QUIC bidi stream | 1 |
| Cursor position | QUIC datagram (TAG_CURSOR) | unreliable |
| Screen frames | moq-lite track | 3 (lowest) |

Invite links encode the iroh `EndpointAddr` (public key + relay URL) and a 16-byte session token. Both peers present the token during handshake — mismatches are rejected. No central server is involved after the initial relay-assisted hole punch.

## Known gaps (next up)

- **Coordinate mapping** — viewer input coordinates are approximate; exact letterbox-aware mapping is in progress
- **Clipboard sync** — implemented in `crates/input`, not wired into session yet
- **Drawing & annotations** — protocol types defined, transport not yet wired
- **Reactions** — protocol types defined, transport not yet wired
- **Mute/unmute** — no way to toggle mic or remote audio mid-session
- **Terminal sharing** — Phase 4; PTY forwarding over QUIC
