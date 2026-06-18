# rust-sharing development commands
# Run `just --list` to see all available commands

# Run all validation steps (the one command agents must run)
all: fmt-check check lint test

# Type-check the workspace
check:
    cargo check --workspace

# Run clippy with warnings as errors
lint:
    cargo clippy --workspace -- -D warnings

# Run all tests
test:
    cargo test --workspace

# Check formatting (non-destructive)
fmt-check:
    cargo fmt --all -- --check

# Apply formatting fixes
fmt:
    cargo fmt --all

# Build in release mode
build:
    cargo build --workspace --release

# Build in debug mode
build-debug:
    cargo build --workspace

# Clean build artifacts
clean:
    cargo clean

# Run a specific crate's tests
test-crate crate:
    cargo test -p {{crate}}

# Check a specific crate
check-crate crate:
    cargo check -p {{crate}}

# Test static FFmpeg linkage — decides P4.5.1 bundling strategy.
# Requires: brew install nasm pkg-config
# If this succeeds and VideoToolbox works in a clean VM → use static.
# If VideoToolbox breaks → use dylibbundler (see Task 4 fallback).
ffmpeg-spike:
    #!/usr/bin/env bash
    set -euo pipefail
    unset FFMPEG_DIR BINDGEN_EXTRA_CLANG_ARGS 2>/dev/null || true
    CARGO_FEATURE_STATIC=1 \
    cargo build -p pairwire \
        --release \
        --features encode,capture,audio,gui,tray \
        2>&1 | tee /tmp/ffmpeg-spike.log
    echo "=== Spike build complete. Check /tmp/ffmpeg-spike.log for errors. ==="
    echo "=== otool -L output follows — check for Homebrew paths: ==="
    otool -L target/release/pairwire | grep -E "libav|libsw|libpostproc|homebrew" || true
