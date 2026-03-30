# AGENTS.md — AI Agent Instructions

## Project Overview

This is a Rust-native peer-to-peer screen sharing and pair programming tool. The core differentiator is terminal-first collaboration over QUIC, with screen sharing, audio, and remote control layered on top.

**Core Stack:**
- **Connectivity**: iroh (P2P QUIC with NAT traversal, relay fallback)
- **Media Transport**: moq-lite + hang (priority-based pub/sub)
- **Screen Capture**: scap (cross-platform: ScreenCaptureKit / Desktop Duplication / PipeWire)
- **Video Encode/Decode**: FFmpeg via ffmpeg-next (hardware-accelerated H.264)
- **Audio**: cpal (capture) + audiopus (Opus codec)
- **UI**: iced or egui + wgpu
- **Serialization**: postcard (compact binary, serde-compatible)
- **Async Runtime**: tokio

## Repository Structure (Target)

```
rust-sharing/
├── AGENTS.md                  # This file
├── CLAUDE.md                  # Claude-specific instructions
├── PLAN.md                    # Phased build plan with agent-ready tasks
├── Cargo.toml                 # Workspace root
├── docs/                      # Research & specifications
│   ├── 01-iroh-noq-connectivity.md
│   ├── 02-moq-lite-vs-moq-transport.md
│   ├── 03-quic-transport-primitives.md
│   ├── 04-screen-capture-encode-pipeline.md
│   ├── 05-concrete-architecture.md
│   └── 06-competitive-analysis.md
├── crates/
│   ├── core/                  # Shared types, config, error types
│   ├── transport/             # iroh + moq-lite integration
│   ├── capture/               # Screen capture (scap) + encoding (FFmpeg)
│   ├── audio/                 # Audio capture (cpal) + Opus codec
│   ├── input/                 # Keyboard, mouse, clipboard forwarding
│   ├── protocol/              # Wire protocol definitions, message types
│   ├── terminal/              # PTY sharing (terminal-first feature)
│   └── app/                   # UI application (iced/egui), binary entry point
└── tests/                     # Integration tests
```

## Coding Conventions

- **Rust edition**: 2024
- **Error handling**: Use `thiserror` for library errors, `anyhow` in the binary/app crate only. Propagate errors with `?`, never `unwrap()` in non-test code.
- **Async**: All network and I/O code is async on tokio. Capture and encode threads use dedicated OS threads with channel-based communication to the tokio runtime.
- **Logging**: Use `tracing` with structured fields. Never `println!` in library crates.
- **Naming**: Follow Rust API guidelines. Crate names use hyphens (`rust-sharing-core`), module names use underscores.
- **Dependencies**: Check `Cargo.toml` before adding any new dependency. Prefer crates already in the workspace. Feature-gate optional functionality.
- **Unsafe**: Avoid unless required for FFI. Document every `unsafe` block with a `// SAFETY:` comment explaining the invariant.
- **Tests**: Unit tests go in `#[cfg(test)] mod tests` within the source file. Integration tests go in `tests/`.
- **Serialization**: Use `postcard` for wire format. Derive `serde::Serialize` and `serde::Deserialize` on all protocol types.
- **Binary size**: Be mindful of dependency bloat. Use feature flags to exclude unused functionality.

## Architecture Principles

1. **Thread model**: Single process, multi-threaded. Dedicated OS threads for capture, encode, decode, and audio. tokio runtime for networking. UI on main thread.
2. **Channel communication**: Use `tokio::sync::mpsc` for async channels, `crossbeam_channel` for low-latency sync channels (cursor datagrams). Ring-buffer semantics where dropping old data is acceptable.
3. **Zero-copy where possible**: Pass frames as `Arc<[u8]>` or `bytes::Bytes`. Avoid cloning large buffers.
4. **Graceful degradation**: Monitor QUIC stats (RTT, loss). Reduce quality (resolution, FPS) under congestion. Audio always takes priority over video.

## Protocol Quick Reference

| Data Type | Transport | Priority | Notes |
|---|---|---|---|
| Audio | moq-lite track | 0 (highest) | 20ms Opus frames |
| Control | Reliable bidirectional stream | 1 | Session mgmt, permissions |
| Keyboard/Mouse | Reliable unidirectional stream | 1 | Must not drop events |
| Cursor position | QUIC datagrams | N/A | Unreliable, 60Hz, ~50 bytes |
| Screen frames | moq-lite track | 3 (lowest) | H.264 NAL units in groups |

## Key Design Decisions

- **iroh for connectivity**: NAT traversal is solved (~90% direct, relay fallback). EndpointId-based addressing for invite links.
- **moq-lite over full moq-transport**: Simpler, sufficient for live streaming, forward-compatible with CDN.
- **FFmpeg for encoding**: No pure-Rust hardware encoding alternative exists. Static linking recommended.
- **Terminal sharing as wedge feature**: PTY forwarding over QUIC streams (not screen capture). Pixel-perfect, near-zero latency, minimal bandwidth.
- **Headphones assumed for MVP**: No echo cancellation in Phase 1-2.

## Agent Task Execution Guidelines

When working on a task from PLAN.md:

1. **Read the relevant docs/** files first to understand the design context.
2. **Check existing crates** before creating new ones. Reuse shared types from `core/`.
3. **Write tests** alongside implementation. At minimum: unit tests for protocol serialization, integration tests for thread communication.
4. **Run `cargo check --workspace`** after every significant change.
5. **Run `cargo clippy --workspace`** before considering a task complete.
6. **Run `cargo test --workspace`** before considering a task complete.
7. **Commit atomically**: One logical change per commit. Reference the PLAN.md task ID in the commit message.
8. **Update PLAN.md**: Mark tasks as completed when done.
