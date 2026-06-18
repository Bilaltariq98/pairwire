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
    APP="${APP:-target/release/pairwire.app}"
    BIN="$APP/Contents/MacOS/pairwire"
    LIBS_DIR="$APP/Contents/libs"
    IDENTITY="${CODESIGN_IDENTITY:--}"
    # Locate FFmpeg dylib directory.
    # FFMPEG_DIR takes precedence; fall back to `brew --prefix ffmpeg`.
    FFMPEG_LIB="${FFMPEG_DIR:-$(brew --prefix ffmpeg 2>/dev/null || echo '')}/lib"
    if [ ! -d "$FFMPEG_LIB" ]; then
        echo "ERROR: FFmpeg lib directory not found ($FFMPEG_LIB). Set FFMPEG_DIR or install via Homebrew."
        exit 1
    fi
    mkdir -p "$LIBS_DIR"
    # dylibbundler 1.x flags:
    #   -b / --bundle-deps   copy transitive dependencies
    #   -x / --fix-file      binary whose load commands to rewrite
    #   -d / --dest-dir      destination directory for copied dylibs
    #   -p / --install-path  @rpath prefix written into each dylib's LC_LOAD_DYLIB
    #   -od / --overwrite-dir erase and recreate dest-dir (idempotent re-runs)
    #   -s / --search-path   extra directory to resolve dylib source files
    # -s $FFMPEG_LIB lets dylibbundler resolve libraries even when the binary
    # load commands already use @executable_path/../libs/ (idempotent re-run).
    dylibbundler \
        -b \
        -x "$BIN" \
        -d "$LIBS_DIR" \
        -p "@executable_path/../libs/" \
        -s "$FFMPEG_LIB" \
        -od
    # Ad-hoc sign each bundled dylib to avoid "mapped file has no cdhash" Gatekeeper error.
    # This error appears when a dylib inside a hardened-runtime .app is unsigned.
    # Bottom-up signing order is mandatory: dylibs first, then the binary, then the .app.
    shopt -s nullglob
    for dylib in "$LIBS_DIR"/*.dylib; do
        codesign \
            --sign "$IDENTITY" \
            --force \
            "$dylib"
        echo "  signed: $(basename "$dylib")"
    done
    if [ -z "$(ls -A "$LIBS_DIR" 2>/dev/null)" ]; then
        echo "ERROR: No dylibs were bundled into $LIBS_DIR — dylibbundler found nothing to copy."
        echo "       Check that the binary links against Homebrew FFmpeg (not static)."
        exit 1
    fi
    # Re-sign the binary (dylibbundler modifies it, invalidating the prior signature).
    codesign \
        --sign "$IDENTITY" \
        --entitlements packaging/pairwire.entitlements \
        --options runtime \
        --force \
        "$BIN"
    # Re-sign the outer bundle bottom-up (after binary is signed).
    codesign \
        --sign "$IDENTITY" \
        --entitlements packaging/pairwire.entitlements \
        --options runtime \
        --force \
        "$APP"
    echo "dylibs bundled and signed in $LIBS_DIR"
    echo "=== Verify inner binary signing: ==="
    codesign -dv "$BIN" 2>&1 | grep -E "Signature|Identifier|Format"

# Build the macOS .app bundle and .dmg using cargo-packager.
# Requires: cargo install cargo-packager --version "0.11" --locked
# Requires: assets/pairwire.icns (run `just icns` first; needs real icon asset)
# Requires: packaging/dmg-background.png (solid-colour placeholder committed in Task 6)
# Requires: FFMPEG_DIR / BINDGEN_EXTRA_CLANG_ARGS set (or CARGO_FEATURE_STATIC=1)
# Output: target/release/pairwire.app
#         target/release/pairwire_*.dmg
package:
    #!/usr/bin/env bash
    set -euo pipefail
    # FFmpeg env — Homebrew path used in local dev; CI sets FFMPEG_DIR from a step.
    FFMPEG_PREFIX="$(brew --prefix ffmpeg 2>/dev/null || echo '')"
    if [ -n "$FFMPEG_PREFIX" ]; then
        export FFMPEG_DIR="$FFMPEG_PREFIX"
        export BINDGEN_EXTRA_CLANG_ARGS="-I${FFMPEG_PREFIX}/include"
        export PKG_CONFIG_PATH="${FFMPEG_PREFIX}/lib/pkgconfig:$(brew --prefix)/lib/pkgconfig"
    fi
    # Step 1: Build the release binary with all production features.
    # cargo-packager 0.11 does not accept --features; build separately first.
    cargo build \
        --release \
        -p pairwire \
        --features encode,capture,audio,gui,tray
    # Step 2: Bundle the pre-built binary into the .app skeleton only.
    # DMG is deliberately deferred to Step 4 so dylibs are included in the archive.
    CODESIGN_IDENTITY="${CODESIGN_IDENTITY:--}" \
    cargo packager \
        --release \
        --formats app \
        -p pairwire
    # Step 3: Bundle FFmpeg dylibs into Contents/libs and rewrite load commands.
    # cargo-packager produces the .app skeleton but does not copy the FFmpeg dylibs,
    # so the produced .app only runs where Homebrew ffmpeg is installed.
    # bundle-dylibs copies libavcodec/libavformat/libavutil/libswscale (and their
    # transitive deps) from $FFMPEG_DIR/lib into Contents/libs, rewrites all
    # @rpath load commands to @executable_path/../libs/ via install_name_tool,
    # ad-hoc codesigns each dylib, then re-codesigns the binary and the .app.
    just bundle-dylibs
    # Step 4: Create the DMG from the now-complete .app (dylibs already bundled).
    # This must run AFTER bundle-dylibs so the DMG archive captures the full bundle.
    CODESIGN_IDENTITY="${CODESIGN_IDENTITY:--}" \
    cargo packager \
        --release \
        --formats dmg \
        -p pairwire
    echo "=== Bundle output ==="
    ls -lh target/release/ | grep -E "pairwire"

# Alias — same as `package` (kept for semantic clarity in docs).
dmg: package

# Stub: sign with Developer ID — no-op until Apple enrollment.
# Set CODESIGN_IDENTITY and APPLE_TEAM_ID env vars to activate.
sign:
    @echo "sign: CODESIGN_IDENTITY=${CODESIGN_IDENTITY:-(not set — ad-hoc only)}"
    @echo "sign: Stub. No-op for ad-hoc/unsigned builds."
    @echo "sign: To enable: set CODESIGN_IDENTITY='Developer ID Application: <Name> (<TeamID>)'"

# Stub: notarize with Apple notary service — requires Developer ID (deferred).
# Uncomment and fill in when Apple enrollment is complete.
notarize:
    @echo "notarize: Stub. Requires Apple Developer ID enrollment (deferred for v1)."
    @echo "notarize: When ready: xcrun notarytool submit target/release/*.dmg"
    @echo "notarize:             --apple-id <email> --team-id <team> --password <app-specific>"
    @echo "notarize:             xcrun stapler staple target/release/pairwire.app"

# Generate pairwire.icns from assets/icon.iconset/icon_1024x1024.png.
# Requires: Xcode CLI tools (sips, iconutil).
# [human] Drop the 1024×1024 PNG at assets/icon.iconset/icon_1024x1024.png first.
icns:
    #!/usr/bin/env bash
    set -euo pipefail
    SRC="assets/icon.iconset/icon_1024x1024.png"
    if [ ! -f "$SRC" ]; then
        echo "ERROR: $SRC not found — provide the 1024×1024 PNG first (task #9)"
        exit 1
    fi
    ICONSET="assets/icon.iconset"
    sips -z 16 16     "$SRC" --out "$ICONSET/icon_16x16.png"
    sips -z 32 32     "$SRC" --out "$ICONSET/icon_16x16@2x.png"
    sips -z 32 32     "$SRC" --out "$ICONSET/icon_32x32.png"
    sips -z 64 64     "$SRC" --out "$ICONSET/icon_32x32@2x.png"
    sips -z 128 128   "$SRC" --out "$ICONSET/icon_128x128.png"
    sips -z 256 256   "$SRC" --out "$ICONSET/icon_128x128@2x.png"
    sips -z 256 256   "$SRC" --out "$ICONSET/icon_256x256.png"
    sips -z 512 512   "$SRC" --out "$ICONSET/icon_256x256@2x.png"
    sips -z 512 512   "$SRC" --out "$ICONSET/icon_512x512.png"
    cp "$SRC"                      "$ICONSET/icon_512x512@2x.png"
    iconutil -c icns "$ICONSET" -o assets/pairwire.icns
    echo "Generated assets/pairwire.icns"
    file assets/pairwire.icns

# Generate a minimal 32x32 RGBA placeholder tray PNG (dark circle on transparent background).
# Replace assets/tray-icon.png with the real 32x32 RGBA design asset before production release.
tray-placeholder:
    #!/usr/bin/env python3
    import struct, zlib, os

    def png_chunk(name, data):
        crc = zlib.crc32(name + data) & 0xffffffff
        return struct.pack('>I', len(data)) + name + data + struct.pack('>I', crc)

    W = 32
    H = 32
    cx = W // 2
    cy = H // 2
    r = 12
    raw = b''
    for y in range(H):
        raw += b'\x00'
        for x in range(W):
            dist2 = (x - cx) ** 2 + (y - cy) ** 2
            if dist2 <= r * r:
                raw += bytes([30, 30, 30, 220])
            else:
                raw += bytes([0, 0, 0, 0])

    compressed = zlib.compress(raw)
    ihdr = struct.pack('>II', W, H) + bytes([8, 6, 0, 0, 0])
    png  = b'\x89PNG\r\n\x1a\n'
    png += png_chunk(b'IHDR', ihdr)
    png += png_chunk(b'IDAT', compressed)
    png += png_chunk(b'IEND', b'')

    os.makedirs('assets', exist_ok=True)
    with open('assets/tray-icon-placeholder.png', 'wb') as f:
        f.write(png)
    print('Written assets/tray-icon-placeholder.png (32x32 RGBA)')

# Verify bundle signing and plist integrity.
verify-bundle:
    #!/usr/bin/env bash
    set -euo pipefail
    APP="target/release/pairwire.app"
    if [ ! -d "$APP" ]; then
        echo "ERROR: $APP not found — run 'just package' first"
        exit 1
    fi
    echo "=== Info.plist lint ==="
    plutil -lint "$APP/Contents/Info.plist"
    echo "=== Inner binary codesign (bottom-up verification) ==="
    codesign -dv "$APP/Contents/MacOS/pairwire" 2>&1 | grep -E "Signature|Identifier|Format"
    echo "=== Outer bundle codesign + entitlements ==="
    codesign -d --entitlements - -vvv "$APP" 2>&1
    echo "=== spctl assessment ==="
    spctl --assess --type execute --verbose "$APP" 2>&1 || true
    echo "=== Version check ==="
    "$APP/Contents/MacOS/pairwire" --version
    echo "=== TCC usage strings ==="
    /usr/libexec/PlistBuddy -c "Print :NSScreenCaptureUsageDescription" "$APP/Contents/Info.plist"
    /usr/libexec/PlistBuddy -c "Print :NSMicrophoneUsageDescription" "$APP/Contents/Info.plist"
    /usr/libexec/PlistBuddy -c "Print :LSMinimumSystemVersion" "$APP/Contents/Info.plist"
    echo "=== Verify complete ==="

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
