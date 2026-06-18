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

# ── Relay infrastructure (Terraform) ──────────────────────────────────────────
# Requires: TF_VAR_hcloud_token, TF_VAR_cloudflare_token, TF_VAR_cloudflare_zone_id,
#           TF_VAR_admin_ip, TF_VAR_ssh_public_key to be set in the environment.
# State backend credentials must also be configured (see infra/terraform/backend.tf).

# Show what Terraform would change (safe, no mutations)
relay-plan:
    cd infra/terraform && terraform init && terraform plan

# Apply the Terraform plan (provisions/updates the relay server) [human approval required]
relay-apply:
    cd infra/terraform && terraform init && terraform apply

# Destroy all relay infrastructure [destructive — human approval required]
relay-destroy:
    cd infra/terraform && terraform init && terraform destroy
