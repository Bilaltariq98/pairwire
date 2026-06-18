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
# pairwire-telemetry uses process-global AtomicBool statics so its tests must
# run single-threaded; all other crates run with default parallelism.
test:
    cargo test -p pairwire-telemetry -- --test-threads=1
    cargo test --workspace --exclude pairwire-telemetry

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
