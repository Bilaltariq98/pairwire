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

# Bundle FFmpeg dylibs into the .app and rewrite rpaths.
# Run AFTER cargo build --release and AFTER cargo-packager creates the .app skeleton.
# Requires: brew install dylibbundler
# APP variable can be overridden: APP=path/to/pairwire.app just bundle-dylibs
bundle-dylibs:
    #!/usr/bin/env bash
    set -euo pipefail
    APP="${APP:-target/release/bundle/macos/pairwire.app}"
    BIN="$APP/Contents/MacOS/pairwire"
    LIBS_DIR="$APP/Contents/libs"
    mkdir -p "$LIBS_DIR"
    dylibbundler \
        --bundle-deps \
        --binary "$BIN" \
        --dest-dir "$LIBS_DIR" \
        --install-path "@rpath/libs/" \
        --overwrite-dir \
        --fix-file "$BIN"
    # Ad-hoc sign each bundled dylib to avoid "mapped file has no cdhash" Gatekeeper error.
    # This error appears when a dylib inside a hardened-runtime .app is unsigned.
    # Bottom-up signing order is mandatory: dylibs first, then the binary, then the .app.
    IDENTITY="${CODESIGN_IDENTITY:--}"
    for dylib in "$LIBS_DIR"/*.dylib; do
        codesign \
            --sign "$IDENTITY" \
            --timestamp \
            --options runtime \
            --force \
            "$dylib"
        echo "  signed: $(basename "$dylib")"
    done
    echo "dylibs bundled and signed in $LIBS_DIR"
    echo "=== Verify inner binary signing: ==="
    codesign -dv "$BIN" 2>&1 | grep -E "Signature|Identifier|Format"

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
