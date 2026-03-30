# Phase 3: Audio + Multi-Platform + Polish

> Goal: Full pair-programming experience with audio, real screen capture, reconnection, and menu bar.
> Estimated effort: 4–6 weeks

## Tasks

### P3.1 — Audio Capture + Opus Encode
- **Status**: 🔲 Not started
- **Crate**: `crates/audio`
- **Key deliverables**: `AudioCaptureThread` (cpal, 48kHz mono), `AudioEncodeThread` (Opus 20ms frames, 32kbps), VAD (silence skip), `OpusPacket` output channel
- **Deps**: `cpal = "0.17"`, `opus = "0.3"` (static feature)

### P3.2 — Opus Decode + cpal Playback
- **Status**: 🔲 Not started
- **Crate**: `crates/audio`
- **Key deliverables**: `AudioDecodeThread` (Opus → f32 PCM), `AudioPlaybackThread` (cpal output stream), simple ring-buffer jitter buffer (40–60ms), volume control
- **Depends on**: P3.1

### P3.3 — Audio moq-lite Transport
- **Status**: 🔲 Not started
- **Crate**: `crates/transport`
- **Key deliverables**: `MoqAudioPublisher` (host, priority 0), `MoqAudioSubscriber` (guest); wire into `session.rs`
- **Depends on**: P3.1, P3.2

### P3.4 — Real Screen Capture (scap)
- **Status**: 🔲 Not started
- **Crate**: `crates/capture`, `crates/app`
- **Key deliverables**: Wire `CaptureThread::start()` into `run_host()` behind `capture` feature; permission check + error message; replace fake source
- **Depends on**: P1.4

### P3.5 — Session Reconnection
- **Status**: 🔲 Not started
- **Crate**: `crates/app`
- **Key deliverables**: Detect connection drop via `conn.closed()`; auto-reconnect loop (3 attempts, exponential backoff); "Reconnecting…" overlay in viewer; "Connection lost" after timeout
- **Depends on**: P2.2

### P3.6 — macOS Menu Bar
- **Status**: 🔲 Not started
- **Crate**: `crates/app`
- **Key deliverables**: `tray-icon` menu bar extra; start/stop sharing, copy invite link, connection status, disconnect; macOS only, behind `tray` feature flag
- **Depends on**: P1.9

### P3.7 — CI: Windows + Linux Builds
- **Status**: 🔲 Not started
- **Key deliverables**: GitHub Actions workflow; `cargo check --workspace` on ubuntu-latest and windows-latest; FFmpeg static link on CI; scap feature gated per platform
- **Depends on**: P1.10

### P3.8 — Clippy Pedantic Pass
- **Status**: 🔲 Not started
- **Key deliverables**: `#![warn(clippy::pedantic)]` on all library crates; fix or `#[allow]` with justification; zero warnings on `cargo clippy --workspace -- -W clippy::pedantic`
