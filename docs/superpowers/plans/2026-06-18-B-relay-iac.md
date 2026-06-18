---
# Workstream B — Self-Hosted iroh-relay + Terraform IaC + Client Wiring

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire pairwire clients to a self-hosted iroh-relay on Hetzner CX22, provisioned reproducibly with Terraform IaC, so the team has full cost/traffic visibility from day 1.

**Architecture:** The iroh-relay binary (buildable via `cargo install iroh-relay --version 0.97.0 --locked --features server`) runs on Hetzner CX22 (EU, nbg1) behind a Hetzner firewall that allows only TCP 443 (WebSocket relay) and UDP 7842 (QAD). The client transport layer reads `PAIRWIRE_RELAY_URL` (or falls back to `https://relay.pairwire.dev`) and builds a merged `RelayMap` that contains both our relay and n0's public relays, so a relay outage degrades gracefully without hard-failing sessions. All infra is codified in `infra/terraform/` so it is reproducible and tear-down-able with no click-ops.

**Tech Stack:** Rust (iroh 0.97 `RelayMode::Custom`, `RelayMap`), Terraform (hetznercloud/hcloud + cloudflare/cloudflare providers), cloud-init YAML (Debian 12, systemd), iroh-relay 0.97.0 binary, Cloudflare DNS, Grafana Cloud (monitoring — human wires alert)

**PLAN.md task ID(s):** P4.5.1 (macOS packaging/distribution phase — relay is the infrastructure prerequisite for this workstream; no dedicated relay task exists in PLAN.md)

---

## File Structure

| Path | Responsibility |
|---|---|
| `crates/transport/src/transport.rs` | **Modify** — `Transport::bind()` accepts optional relay URL, builds merged `RelayMap` |
| `crates/core/src/config.rs` | **Modify** — add `relay_url: Option<String>` to a new `NetworkConfig`; add `NetworkConfig::validate()` |
| `crates/core/src/lib.rs` | **Modify** — re-export `NetworkConfig` |
| `infra/terraform/providers.tf` | **Create** — provider pins (hcloud 1.x, cloudflare 4.x) |
| `infra/terraform/variables.tf` | **Create** — all input variables (tokens, zone, admin IP, ssh key) |
| `infra/terraform/backend.tf` | **Create** — Terraform Cloud or Cloudflare R2 state backend |
| `infra/terraform/main.tf` | **Create** — hcloud ssh key, firewall, server with cloud-init |
| `infra/terraform/dns.tf` | **Create** — Cloudflare A + AAAA records for relay.pairwire.dev |
| `infra/terraform/outputs.tf` | **Create** — relay_url, server_ip_v4, server_ip_v6 |
| `infra/terraform/cloud-init/relay.yaml` | **Create** — installs iroh-relay, writes config + systemd unit |
| `justfile` | **Modify** — add `relay-plan`, `relay-apply`, `relay-destroy` recipes |
| `LEARNINGS.md` | **Modify** — add entry about iroh 0.97 `RelayMap::extend` pattern for merged relay maps |

---

## Tasks

### Task 1: Add `NetworkConfig` to `crates/core` [agent]

`NetworkConfig` is the shared config type for relay URL — it lives in `crates/core` (shared types) so both the transport crate and the app crate can read it without circular deps.

**Files:**
- Modify: `crates/core/src/config.rs` (add after line 167, before the `#[cfg(test)]` block)
- Modify: `crates/core/src/lib.rs` (re-export)

#### Steps

- [ ] **Step 1.1 — Write failing test for `NetworkConfig` validation** [agent]

  Add to `crates/core/src/config.rs` inside the existing `#[cfg(test)] mod tests` block, before the closing brace:

  ```rust
  // --- NetworkConfig validation ---

  #[test]
  fn network_config_default_is_valid() {
      NetworkConfig::default().validate().unwrap();
  }

  #[test]
  fn network_config_rejects_empty_relay_url() {
      let cfg = NetworkConfig {
          relay_url: Some(String::new()),
      };
      assert!(cfg.validate().is_err());
  }

  #[test]
  fn network_config_rejects_invalid_scheme() {
      let cfg = NetworkConfig {
          relay_url: Some("ftp://relay.example.com".into()),
      };
      assert!(cfg.validate().is_err());
  }

  #[test]
  fn network_config_accepts_https_url() {
      let cfg = NetworkConfig {
          relay_url: Some("https://relay.pairwire.dev".into()),
      };
      cfg.validate().unwrap();
  }

  #[test]
  fn network_config_accepts_none() {
      let cfg = NetworkConfig { relay_url: None };
      cfg.validate().unwrap();
  }

  #[test]
  fn network_config_round_trip() {
      let cfg = NetworkConfig {
          relay_url: Some("https://relay.pairwire.dev".into()),
      };
      let bytes = postcard::to_allocvec(&cfg).unwrap();
      let decoded: NetworkConfig = postcard::from_bytes(&bytes).unwrap();
      assert_eq!(decoded, cfg);
  }
  ```

  Run the test (expected FAIL — `NetworkConfig` doesn't exist yet):

  ```bash
  cargo test -p pairwire-core network_config 2>&1 | head -20
  # Expected: error[E0422]: cannot find type `NetworkConfig` in this scope
  ```

- [ ] **Step 1.2 — Implement `NetworkConfig`** [agent]

  Add to `crates/core/src/config.rs` immediately before the `#[cfg(test)]` block (after the `AudioConfig` impl block):

  ```rust
  /// The default relay URL used when `PAIRWIRE_RELAY_URL` is not set.
  pub const DEFAULT_RELAY_URL: &str = "https://relay.pairwire.dev";

  /// Network connectivity configuration.
  ///
  /// Controls which iroh-relay servers the transport layer contacts.
  /// The custom relay is **merged** with n0's public relays (not a replacement),
  /// so a relay outage degrades gracefully instead of hard-failing sessions.
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct NetworkConfig {
      /// Custom relay URL to prepend to the relay map.
      ///
      /// Must be an `https://` URL (iroh-relay uses TLS WebSocket since 0.91).
      /// When `None`, only n0's public relays are used.
      ///
      /// Override at runtime with the `PAIRWIRE_RELAY_URL` environment variable.
      pub relay_url: Option<String>,
  }

  impl NetworkConfig {
      /// Validate the network configuration.
      ///
      /// Returns an error if `relay_url` is set to an empty string or a URL
      /// whose scheme is not `https`.
      pub fn validate(&self) -> crate::Result<()> {
          if let Some(url) = &self.relay_url {
              if url.is_empty() {
                  return Err(crate::Error::InvalidConfig(
                      "NetworkConfig: relay_url must not be empty (set to None to use default)"
                          .into(),
                  ));
              }
              let scheme = url
                  .split_once("://")
                  .map(|(s, _)| s)
                  .unwrap_or_default();
              if scheme != "https" {
                  return Err(crate::Error::InvalidConfig(format!(
                      "NetworkConfig: relay_url must use https:// scheme, got '{scheme}://'"
                  )));
              }
          }
          Ok(())
      }

      /// Resolve the effective relay URL, preferring the `PAIRWIRE_RELAY_URL`
      /// environment variable over the config field, and the config field over
      /// the compiled-in default.
      ///
      /// Returns `None` only if the env var is set to the empty string (explicit
      /// opt-out of a custom relay).
      pub fn effective_relay_url() -> Option<String> {
          match std::env::var("PAIRWIRE_RELAY_URL") {
              Ok(val) if val.is_empty() => None,
              Ok(val) => Some(val),
              Err(_) => Some(DEFAULT_RELAY_URL.to_owned()),
          }
      }
  }

  impl Default for NetworkConfig {
      fn default() -> Self {
          Self {
              relay_url: Some(DEFAULT_RELAY_URL.to_owned()),
          }
      }
  }
  ```

- [ ] **Step 1.3 — Re-export `NetworkConfig` and `DEFAULT_RELAY_URL`** [agent]

  `crates/core/src/lib.rs` currently contains (verified):
  ```rust
  pub use config::{AudioConfig, CaptureConfig, SessionConfig};
  pub use error::{Error, Result};
  pub use types::{PeerId, SessionId};
  ```

  Replace the `config` re-export line only (leave the others intact):
  ```rust
  pub use config::{AudioConfig, CaptureConfig, NetworkConfig, SessionConfig, DEFAULT_RELAY_URL};
  ```

  **Note:** `VideoEncoder` is intentionally NOT added to this re-export. It is not currently re-exported from `lib.rs` and adding it here would be a behavior-changing side-effect beyond the scope of this task. Any code needing `VideoEncoder` should use the full path `pairwire_core::config::VideoEncoder`.

- [ ] **Step 1.4 — Run tests (expected PASS)** [agent]

  ```bash
  cargo test -p pairwire-core network_config 2>&1
  # Expected output:
  # test tests::network_config_accepts_https_url ... ok
  # test tests::network_config_accepts_none ... ok
  # test tests::network_config_default_is_valid ... ok
  # test tests::network_config_rejects_empty_relay_url ... ok
  # test tests::network_config_rejects_invalid_scheme ... ok
  # test tests::network_config_round_trip ... ok
  # test result: ok. 6 passed; 0 failed
  ```

- [ ] **Step 1.5 — Verify full core test suite still passes** [agent]

  ```bash
  cargo test -p pairwire-core 2>&1 | tail -5
  # Expected: test result: ok. N passed; 0 failed; 0 ignored
  ```

- [ ] **Step 1.6 — Commit** [agent]

  ```bash
  git add crates/core/src/config.rs crates/core/src/lib.rs
  git commit -m "feat(P4.5.1): add NetworkConfig + relay URL validation to pairwire-core"
  ```

---

### Task 2: Wire `RelayMode::Custom` into `Transport::bind()` [agent]

**Files:**
- Modify: `crates/transport/src/transport.rs` — add relay URL resolution and `RelayMap` merge

**Key iroh 0.97 API facts (verified from source):**
- `iroh::RelayMode` is `pub enum RelayMode { Disabled, Default, Staging, Custom(RelayMap) }`
- `RelayMode::custom(urls: impl IntoIterator<Item = RelayUrl>)` builds a `Custom` variant from URLs
- `Builder::relay_mode(mode: RelayMode) -> Self` replaces the relay config (not additive)
- `iroh::RelayMap::extend(&self, other: &RelayMap)` merges two maps in-place
- `iroh::defaults::prod::default_relay_map()` returns the n0 production relay map
- `iroh::RelayUrl` is from `iroh_base`, re-exported as `iroh::RelayUrl`

**Strategy for "custom relay + n0 public relays as fallback":** `Builder::relay_mode` is a replacement, not an additive call. To merge, build the custom `RelayMap`, call `.extend(&prod_map)` on it, then pass `RelayMode::Custom(merged)` to `relay_mode()`. This gives us our relay first (clients will attempt it) plus n0's as fallback. The tradeoff: if our relay is down, hole-punch regression (LEARNINGS 2026-03-30 v0.96) may keep some sessions on n0's public relays permanently until reconnect — acceptable for beta.

#### Steps

- [ ] **Step 2.1 — Write failing unit test** [agent]

  In `crates/transport/src/transport.rs`, add at the bottom of the file before `}` of the module (or inside a test module if one exists — check the current file first):

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use iroh::defaults::prod as iroh_prod_defaults;

      /// Verify that an https:// relay URL is accepted without error.
      /// This test does NOT bind a real endpoint — it only exercises the URL
      /// parsing / RelayMap construction path.
      #[test]
      fn relay_url_valid_https_parses() {
          let url: iroh::RelayUrl = "https://relay.pairwire.dev"
              .parse()
              .expect("valid https relay URL should parse");
          // The RelayMap should contain exactly our URL.
          let map = iroh::RelayMap::from_iter([url.clone()]);
          assert!(map.contains(&url));
      }

      /// Verify that an invalid (non-https) URL is rejected at the RelayUrl parse step.
      #[test]
      fn relay_url_invalid_scheme_rejected() {
          let result: Result<iroh::RelayUrl, _> = "ftp://relay.example.com".parse();
          assert!(result.is_err(), "non-https URL must be rejected by RelayUrl::parse");
      }

      /// Verify that merging our custom map with the n0 prod map yields a map
      /// that contains both our relay and at least one n0 relay.
      /// The prod map has 4 nodes (NA-east, NA-west, EU, AP), so merged is 5.
      #[test]
      fn relay_map_merge_contains_both() {
          let our_url: iroh::RelayUrl = "https://relay.pairwire.dev".parse()
              .expect("test relay URL must parse");
          let custom = iroh::RelayMap::from_iter([our_url.clone()]);
          let prod = iroh_prod_defaults::default_relay_map();
          custom.extend(&prod);
          // Our relay is present.
          assert!(custom.contains(&our_url), "custom relay must be in merged map");
          // At least one n0 relay is present (prod map has 4 nodes).
          assert!(custom.len() > 1, "merged map must contain n0 relays too");
      }
  }
  ```

  Run (these are pure unit tests — no network, no endpoint binding needed):

  ```bash
  cargo test -p pairwire-transport relay_url 2>&1 | head -30
  # Expected: test tests::relay_url_valid_https_parses ... ok
  # Expected: test tests::relay_url_invalid_scheme_rejected ... ok
  cargo test -p pairwire-transport relay_map_merge 2>&1 | head -30
  # Expected: test tests::relay_map_merge_contains_both ... ok
  ```

  If the tests fail to compile, the most likely cause is that `iroh::RelayMap::from_iter` or `iroh::defaults::prod` is not available — check the iroh version in `crates/transport/Cargo.toml` first (`grep iroh crates/transport/Cargo.toml`). Do not add a new version pin without checking; use whatever is already in the workspace.

- [ ] **Step 2.2 — Update `Transport::bind()` to accept an optional relay URL** [agent]

  Replace the current `Transport::bind()` signature and body in `crates/transport/src/transport.rs`.

  Current signature:
  ```rust
  pub async fn bind() -> Result<Self> {
  ```

  New signature and imports. Add to the top-level `use` block in `transport.rs` (the existing `use iroh::{Endpoint, endpoint::{...}}` line is already there — add a new `use` line below it):
  ```rust
  use iroh::{RelayMap, RelayMode, RelayUrl};
  use iroh::defaults::prod as iroh_prod_defaults;
  ```

  `iroh::defaults::prod` is a module path, not a type. It is not imported implicitly by `use iroh::*` — the fully-qualified module must be brought into scope explicitly. The call `iroh_prod_defaults::default_relay_map()` in `bind()` below is the only use site.

  Replace the `pub async fn bind() -> Result<Self>` function entirely with:

  ```rust
  /// Bind a new iroh endpoint with an optional custom relay.
  ///
  /// The relay URL is resolved in this order:
  /// 1. `relay_url` parameter (if `Some`)
  /// 2. `PAIRWIRE_RELAY_URL` environment variable
  /// 3. Compiled-in default `https://relay.pairwire.dev`
  ///
  /// The custom relay is **merged** with n0's public production relays so that
  /// a relay outage degrades (slower hole-punch, more relaying via n0) rather
  /// than hard-failing. Pass `relay_url = Some("")` to use n0 relays only (not
  /// recommended for production).
  ///
  /// # Errors
  ///
  /// Returns an error if the relay URL cannot be parsed or the endpoint cannot
  /// be bound (e.g., port conflict).
  #[instrument(name = "transport_bind")]
  pub async fn bind(relay_url: Option<&str>) -> Result<Self> {
      let idle_timeout = IdleTimeout::try_from(TRANSPORT_MAX_IDLE_TIMEOUT)
          .map_err(|e| Error::Transport(format!("invalid idle timeout: {e}")))?;

      let transport_config = QuicTransportConfig::builder()
          .keep_alive_interval(TRANSPORT_KEEP_ALIVE_INTERVAL)
          .max_idle_timeout(Some(idle_timeout))
          .build();

      // Resolve effective relay URL: parameter → env var → default.
      let effective_url = relay_url
          .map(|s| s.to_owned())
          .or_else(pairwire_core::NetworkConfig::effective_relay_url);

      let relay_mode = if let Some(url_str) = effective_url {
          let parsed: RelayUrl = url_str.parse().map_err(|e| {
              Error::Transport(format!("invalid relay URL '{url_str}': {e}"))
          })?;
          // Build our custom map, then extend with n0 prod relays for fallback.
          let custom_map = RelayMap::from_iter([parsed.clone()]);
          let prod_map = iroh_prod_defaults::default_relay_map();
          custom_map.extend(&prod_map);
          tracing::info!(
              relay_url = %parsed,
              total_relays = custom_map.len(),
              "using merged relay map (custom + n0 prod)"
          );
          RelayMode::Custom(custom_map)
      } else {
          tracing::info!("relay_url explicitly disabled; using n0 public relays only");
          RelayMode::Default
      };

      let endpoint = Endpoint::builder(presets::N0)
          .relay_mode(relay_mode)
          .alpns(vec![ALPN.to_vec()])
          .transport_config(transport_config)
          .bind()
          .await
          .map_err(|e| Error::Transport(format!("failed to bind endpoint: {e}")))?;

      info!(
          endpoint_id = %endpoint.id(),
          "iroh endpoint bound"
      );

      Ok(Self { endpoint })
  }
  ```

  **Note:** `presets::N0` already calls `builder.relay_mode(default_relay_mode())` inside its `apply()`. Our subsequent `.relay_mode(relay_mode)` call on the builder **replaces** the preset's relay config (the builder method overwrites the `TransportConfig::Relay` entry in `self.transports`). This is the correct and intended behavior — verified in iroh 0.97 `endpoint.rs:516-534`.

- [ ] **Step 2.3 — Update all callers of `Transport::bind()`** [agent]

  There are exactly two call sites (verified from codebase):
  - `crates/app/src/session.rs:125` — inside `run_host_session()`
  - `crates/app/src/session.rs:1106` — inside `run_guest_session()`

  Neither function has a `NetworkConfig` parameter yet. Pass `None` to use the default relay URL resolution (env var `PAIRWIRE_RELAY_URL`, then compiled-in default `https://relay.pairwire.dev`):

  In `crates/app/src/session.rs` at line 125, replace:
  ```rust
  let transport = Transport::bind()
      .await
      .context("failed to bind iroh endpoint")?;
  ```
  with:
  ```rust
  // Relay URL resolved from PAIRWIRE_RELAY_URL env var or compiled default.
  let transport = Transport::bind(None)
      .await
      .context("failed to bind iroh endpoint")?;
  ```

  At line 1106, replace the same pattern identically.

  Confirm no other call sites remain:
  ```bash
  grep -rn "Transport::bind" /Users/bilal/Documents/personal/Github/rust-sharing/crates/ --include="*.rs"
  # Expected: only the two lines above, now showing Transport::bind(None)
  ```

- [ ] **Step 2.4 — Run transport tests** [agent]

  ```bash
  cargo test -p pairwire-transport 2>&1 | tail -10
  # Expected: test result: ok. N passed; 0 failed
  ```

- [ ] **Step 2.5 — Run workspace check** [agent]

  ```bash
  cargo check --workspace 2>&1 | tail -5
  # Expected: no errors
  ```

- [ ] **Step 2.6 — Commit** [agent]

  ```bash
  git add crates/transport/src/transport.rs crates/app/src/
  git commit -m "feat(P4.5.1): wire RelayMode::Custom + n0 fallback into Transport::bind"
  ```

---

### Task 3: Update LEARNINGS.md with the relay map merge pattern [agent]

**Files:**
- Modify: `LEARNINGS.md`

#### Steps

- [ ] **Step 3.1 — Append learning entry** [agent]

  Add at the end of `LEARNINGS.md`:

  ```markdown
  ### 2026-06-18 — iroh 0.97 / iroh-relay 0.97.0: RelayMap::extend() is the correct way to merge custom + n0 relays

  - **Area**: iroh, transport, relay
  - **What**: `Builder::relay_mode(mode)` is a replacement, not additive. Calling it twice sets the relay to the second value only. To combine a custom relay with n0's public relays, you must build the two `RelayMap`s separately and merge them with `custom_map.extend(&prod_map)` before wrapping in `RelayMode::Custom(merged)`. The `RelayMap::extend` method acquires a write lock on the inner `BTreeMap` and inserts all entries from `other`.
  - **Resolution**: See `crates/transport/src/transport.rs` `Transport::bind()`. The `effective_url` path builds `custom_map` from a single parsed `RelayUrl`, extends it with `iroh_prod_defaults::default_relay_map()`, then passes `RelayMode::Custom(custom_map)`. The `presets::N0` relay mode set by the preset is overwritten by our subsequent `.relay_mode()` call — this is intentional and verified in iroh-0.97.0/src/endpoint.rs:516-534.
  - **Key insight**: `iroh::defaults::prod` is a module path, not a type. It is NOT implicitly available. Use `use iroh::defaults::prod as iroh_prod_defaults;` and call `iroh_prod_defaults::default_relay_map()`. Calling `iroh::defaults::prod::default_relay_map()` as a fully-qualified path (without a use statement) also works but is verbose.
  ```

- [ ] **Step 3.2 — Commit** [agent]

  ```bash
  git add LEARNINGS.md
  git commit -m "docs(P4.5.1): document RelayMap::extend merge pattern for custom + n0 fallback"
  ```

---

### Task 4: Provision accounts and secrets [human]

Before any `terraform plan` or `apply` can run, these accounts and tokens must exist. None of them can be automated — they require payment, ToS acceptance, or key generation.

**Files:** none (secrets stored in password manager + GitHub Actions secrets)

| # | Action | Output needed |
|---|---|---|
| 1 | **Register `pairwire.dev`** via Cloudflare Registrar (~$12/yr, supports `.dev`). This is a hard prerequisite for DNS record creation in `dns.tf` and for ACME cert issuance (hostname = `relay.pairwire.dev`). Payment required. | Domain active + nameservers pointing to Cloudflare |
| 2 | Create Hetzner Cloud account → create project → generate API token (Read+Write) | `TF_VAR_hcloud_token` |
| 3 | On the Cloudflare account from step 1 (domain already added zone) → generate API token scoped to DNS edit on that zone → copy the Zone ID | `TF_VAR_cloudflare_token`, `TF_VAR_cloudflare_zone_id` |
| 4 | Generate (or reuse) an SSH keypair → copy the public key | `TF_VAR_ssh_public_key` |
| 7 | Choose state backend: Terraform Cloud (free, easy) or Cloudflare R2 (advanced) → obtain credentials | Terraform Cloud token **or** R2 access key + secret + bucket name |

**Blocking order:** Step 1 (domain registration) must complete before `terraform apply` can create DNS records and before ACME can issue a cert for `relay.pairwire.dev`. All other steps are parallelizable with the Rust code tasks (Tasks 1–3).

**Recommended storage:** 1Password → GitHub Actions secrets → local `.env` (never committed). See `infra/terraform/variables.tf` for variable names.

---

### Task 5: Terraform — `infra/terraform/` skeleton [agent]

Create the `infra/terraform/` directory and all `.tf` files. The state backend uses Terraform Cloud (free tier). To switch to Cloudflare R2, see the comment in `backend.tf`.

**Files:**
- Create: `infra/terraform/providers.tf`
- Create: `infra/terraform/variables.tf`
- Create: `infra/terraform/backend.tf`

#### Steps

- [ ] **Step 5.1 — Create `infra/terraform/providers.tf`** [agent]

  ```hcl
  # infra/terraform/providers.tf
  # Provider version pins for reproducibility.
  # Update these only deliberately — changing provider versions can require
  # a `terraform init -upgrade` and may break existing state.

  terraform {
    required_version = ">= 1.9.0"

    required_providers {
      hcloud = {
        source  = "hetznercloud/hcloud"
        version = "~> 1.49"
      }
      cloudflare = {
        source  = "cloudflare/cloudflare"
        version = "~> 4.43"
      }
    }
  }

  provider "hcloud" {
    token = var.hcloud_token
  }

  provider "cloudflare" {
    api_token = var.cloudflare_token
  }
  ```

- [ ] **Step 5.2 — Create `infra/terraform/variables.tf`** [agent]

  ```hcl
  # infra/terraform/variables.tf
  # All sensitive variables are marked sensitive = true.
  # Pass values via TF_VAR_* environment variables or a .tfvars file
  # that is listed in .gitignore. Never commit real values.

  variable "hcloud_token" {
    description = "Hetzner Cloud API token with read/write permissions."
    type        = string
    sensitive   = true
  }

  variable "cloudflare_token" {
    description = "Cloudflare API token with DNS:Edit permission on pairwire.dev zone."
    type        = string
    sensitive   = true
  }

  variable "cloudflare_zone_id" {
    description = "Cloudflare Zone ID for pairwire.dev."
    type        = string
    sensitive   = false
  }

  variable "admin_ip" {
    description = <<-EOT
      Your current egress IPv4 address, in CIDR notation (e.g. 1.2.3.4/32).
      This is the only IP allowed to SSH into the relay server.
      Find it with: curl -s https://checkip.amazonaws.com && echo /32
    EOT
    type        = string

    validation {
      condition     = can(cidrnetmask(var.admin_ip))
      error_message = "admin_ip must be a valid CIDR block (e.g. 1.2.3.4/32)."
    }
  }

  variable "ssh_public_key" {
    description = "SSH public key material (content of ~/.ssh/id_ed25519.pub or similar)."
    type        = string
    sensitive   = false
  }

  variable "server_location" {
    description = "Hetzner datacenter location. nbg1 (Nuremberg) or fsn1 (Falkenstein) for EU."
    type        = string
    default     = "nbg1"

    validation {
      condition     = contains(["nbg1", "fsn1", "hel1", "ash", "hil"], var.server_location)
      error_message = "server_location must be a valid Hetzner location slug."
    }
  }
  ```

- [ ] **Step 5.3 — Create `infra/terraform/backend.tf`** [agent]

  **Option A (recommended): Terraform Cloud**

  ```hcl
  # infra/terraform/backend.tf
  # State is stored in Terraform Cloud (free tier, up to 500 resources).
  # To bootstrap:
  #   1. Create a Terraform Cloud account at https://app.terraform.io
  #   2. Create an organization and a workspace named "pairwire-relay"
  #   3. Set workspace execution mode to "Local"
  #   4. Run: terraform login  (stores token in ~/.terraform.d/credentials.tfrc.json)
  #   5. Run: terraform init
  #
  # The workspace token is in ~/.terraform.d/ — never commit it.
  # For CI, set TF_TOKEN_app_terraform_io as a GitHub Actions secret.

  terraform {
    backend "remote" {
      hostname     = "app.terraform.io"
      organization = "pairwire"

      workspaces {
        name = "pairwire-relay"
      }
    }
  }
  ```

  **Option B (advanced): Cloudflare R2 — comment the above and uncomment this:**

  ```hcl
  # terraform {
  #   backend "s3" {
  #     # Cloudflare R2 presents an S3-compatible API.
  #     bucket                      = "pairwire-tfstate"
  #     key                         = "relay/terraform.tfstate"
  #     region                      = "auto"
  #     endpoint                    = "https://<ACCOUNT_ID>.r2.cloudflarestorage.com"
  #     access_key                  = "<R2_ACCESS_KEY_ID>"          # via TF_VAR or env
  #     secret_key                  = "<R2_SECRET_ACCESS_KEY>"      # via TF_VAR or env
  #     skip_credentials_validation = true
  #     skip_metadata_api_check     = true
  #     skip_region_validation      = true
  #     force_path_style            = true
  #   }
  # }
  ```

- [ ] **Step 5.4 — Create `infra/terraform/.gitignore`** [agent]

  Terraform generates state files, lock files, and provider binaries that must never be committed. Create `infra/terraform/.gitignore` with:

  ```gitignore
  # Terraform state — never commit; state contains plaintext resource IDs and may contain secrets
  *.tfstate
  *.tfstate.backup
  .terraform.tfstate.lock.info

  # Terraform working directory (provider binaries, module cache)
  .terraform/

  # Variable overrides — may contain real secrets
  *.tfvars
  *.tfvars.json

  # Crash log (contains stack traces, never useful in git)
  crash.log
  crash.*.log

  # Generated plan output files
  *.tfplan
  ```

  Verify:
  ```bash
  cat /Users/bilal/Documents/personal/Github/rust-sharing/infra/terraform/.gitignore
  # Expected: file contents as above
  ```

- [ ] **Step 5.5 — Verify syntax** [agent] (no tokens needed)

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing/infra/terraform
  terraform fmt -check .
  # Expected: no output (all files correctly formatted)
  # If differences: run terraform fmt . then re-check
  terraform init -backend=false
  # Expected: Terraform has been successfully initialized!
  terraform validate
  # Expected: Success! The configuration is valid.
  ```

- [ ] **Step 5.6 — Commit** [agent]

  ```bash
  git add infra/terraform/providers.tf infra/terraform/variables.tf \
          infra/terraform/backend.tf infra/terraform/.gitignore
  git commit -m "feat(P4.5.1): add Terraform providers, variables, state backend config, and .gitignore"
  ```

---

### Task 6: Terraform — `main.tf`, `dns.tf`, `outputs.tf` [agent]

**Files:**
- Create: `infra/terraform/main.tf`
- Create: `infra/terraform/dns.tf`
- Create: `infra/terraform/outputs.tf`
- Create: `infra/terraform/cloud-init/relay.yaml`

#### Steps

- [ ] **Step 6.1 — Create cloud-init template `infra/terraform/cloud-init/relay.yaml`** [agent]

  The iroh-relay 0.97.0 binary is not distributed as a pre-built release artifact on GitHub (the iroh project ships source only via crates.io). It is installed via `cargo install iroh-relay --version 0.97.0 --locked --features server`. This adds ~3-4 min to first boot but requires no external artifact hosting or version pinning beyond the crate version. `--locked` is safe here because `cargo install` synthesises a lock from the registry index — it does NOT require a `Cargo.lock` file on disk. Pin to `0.97.0` explicitly to match the workspace.

  ```yaml
  #cloud-config
  # cloud-init for pairwire relay server (Hetzner CX22, Debian 12)
  # Installs iroh-relay 0.97.0 and starts it as a systemd service with
  # ACME TLS for relay.pairwire.dev.
  #
  # First boot takes ~5-8 minutes (Rust toolchain + cargo install).
  # Subsequent reboots are fast (binary is cached at /usr/local/bin/iroh-relay).

  packages:
    - curl
    - ca-certificates
    - build-essential
    - pkg-config
    - libssl-dev
    - git

  runcmd:
    # Install Rust via rustup (non-interactive, stable toolchain)
    - |
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain stable --profile minimal
    - . /root/.cargo/env
    # Install iroh-relay with the server feature enabled.
    # Pinned to 0.97.0 to match workspace Cargo.lock. Bump deliberately.
    # --locked is safe: cargo install synthesises a lock from the registry index
    # without requiring a Cargo.lock file on disk.
    - /root/.cargo/bin/cargo install iroh-relay --version 0.97.0 --locked --features server
    - cp /root/.cargo/bin/iroh-relay /usr/local/bin/iroh-relay
    # Write the relay configuration
    - mkdir -p /etc/iroh-relay /var/lib/iroh-relay/acme-cache
    - |
      cat > /etc/iroh-relay/iroh-relay.toml <<'TOML'
      # iroh-relay 0.97.0 configuration
      # Reference: iroh-relay-0.97.0/src/main.rs Config struct

      # Enable the relay proxy (required for connectivity fallback)
      enable_relay = true

      # HTTP redirect / captive-portal on port 80.
      # Cloud-init does not open port 80 — rely on Hetzner firewall for 443.
      http_bind_addr = "[::]:80"

      # Enable Prometheus-compatible metrics endpoint on :9090
      enable_metrics = true
      metrics_bind_addr = "[::]:9090"

      # Enable QAD (QUIC Address Discovery) — recommended, port 7842 UDP
      enable_quic_addr_discovery = true

      [tls]
      # HTTPS/WSS relay on port 443
      https_bind_addr = "[::]:443"
      # QAD QUIC on port 7842
      quic_bind_addr = "[::]:7842"
      # TLS via Let's Encrypt ACME.
      # IMPORTANT: CertMode serde representation is the Rust variant name as-is
      # (no rename_all). The correct value is "LetsEncrypt" (capital L, capital E),
      # NOT "letsencrypt". Using the wrong casing causes config parse failure + exit.
      cert_mode = "LetsEncrypt"
      hostname = "relay.pairwire.dev"
      cert_dir = "/var/lib/iroh-relay/acme-cache"
      prod_tls = true
      # IMPORTANT: set this to a real email so LE can contact you for expiry notices.
      contact = "ops@pairwire.dev"

      # Access control: open relay (anyone can use it).
      # Cost-cap: Hetzner firewall + egress alert at 15 TB/mo.
      # iroh-relay has no per-node rate limiting (issue #2967).
      #
      # IMPORTANT: `access` is a direct root-level field (type AccessConfig enum),
      # NOT a sub-table. Do NOT use [access] section syntax — that would deserialize
      # as a struct and crash config parsing. Correct form is a bare key at root level:
      access = "everyone"
      TOML
    # Write systemd unit
    - |
      cat > /etc/systemd/system/iroh-relay.service <<'UNIT'
      [Unit]
      Description=iroh relay server for pairwire
      Documentation=https://iroh.computer/docs
      After=network-online.target
      Wants=network-online.target

      [Service]
      Type=simple
      User=root
      ExecStart=/usr/local/bin/iroh-relay --config-path /etc/iroh-relay/iroh-relay.toml
      Restart=always
      RestartSec=5
      # Allow binding to port 443 as root (CX22 runs as root)
      AmbientCapabilities=CAP_NET_BIND_SERVICE
      # Log to journald
      StandardOutput=journal
      StandardError=journal
      SyslogIdentifier=iroh-relay

      [Install]
      WantedBy=multi-user.target
      UNIT
    - systemctl daemon-reload
    - systemctl enable iroh-relay.service
    - systemctl start iroh-relay.service

  # Ensure the relay service starts even after reboot
  bootcmd: []
  ```

- [ ] **Step 6.2 — Create `infra/terraform/main.tf`** [agent]

  ```hcl
  # infra/terraform/main.tf
  # Provisions the Hetzner relay server for relay.pairwire.dev.

  locals {
    # Read cloud-init template from file.
    # ${path.module} is the directory containing this .tf file.
    cloud_init_rendered = file("${path.module}/cloud-init/relay.yaml")
  }

  # ── SSH Key ──────────────────────────────────────────────────────────────────

  resource "hcloud_ssh_key" "pairwire_relay" {
    name       = "pairwire-relay-key"
    public_key = var.ssh_public_key

    labels = {
      managed-by = "terraform"
      project    = "pairwire"
    }
  }

  # ── Firewall ─────────────────────────────────────────────────────────────────
  # Default-deny inbound. Allow only:
  #   - TCP 443   — iroh-relay WebSocket (relay.pairwire.dev)
  #   - UDP 7842  — QUIC Address Discovery (QAD)
  #   - TCP 22    — SSH from admin IP only
  # All other inbound traffic is dropped.
  # Note: STUN (UDP 3478) is NOT needed — iroh retired STUN in 0.90.
  # Note: Prometheus metrics (TCP 9090) are NOT opened externally —
  #       Grafana Cloud uses a push model (alloy/agent) or we SSH-tunnel.

  resource "hcloud_firewall" "pairwire_relay" {
    name = "pairwire-relay-fw"

    # TCP 443 — WebSocket relay — open to all (relay must be reachable by peers)
    rule {
      direction   = "in"
      protocol    = "tcp"
      port        = "443"
      source_ips  = ["0.0.0.0/0", "::/0"]
      description = "iroh-relay WebSocket (HTTPS)"
    }

    # UDP 7842 — QAD — open to all
    rule {
      direction   = "in"
      protocol    = "udp"
      port        = "7842"
      source_ips  = ["0.0.0.0/0", "::/0"]
      description = "QUIC Address Discovery (QAD)"
    }

    # TCP 22 — SSH — admin only. The firewall IP allowlist is the cost-cap knob:
    # removing all public access stops the relay. Adding more IPs opens it wider.
    rule {
      direction   = "in"
      protocol    = "tcp"
      port        = "22"
      source_ips  = [var.admin_ip]
      description = "SSH admin access"
    }

    # Outbound: allow all (required for ACME validation, cargo install, DNS, etc.)
    # IMPORTANT: for outbound TCP/UDP rules that allow all ports, the `port`
    # attribute must be OMITTED entirely — the hcloud provider 1.x rejects
    # port = "any" as an invalid value. Omitting port means all ports.
    rule {
      direction       = "out"
      protocol        = "tcp"
      destination_ips = ["0.0.0.0/0", "::/0"]
    }
    rule {
      direction       = "out"
      protocol        = "udp"
      destination_ips = ["0.0.0.0/0", "::/0"]
    }
    rule {
      direction       = "out"
      protocol        = "icmp"
      destination_ips = ["0.0.0.0/0", "::/0"]
    }

    labels = {
      managed-by = "terraform"
      project    = "pairwire"
    }
  }

  # ── Server ───────────────────────────────────────────────────────────────────

  resource "hcloud_server" "pairwire_relay" {
    name        = "pairwire-relay"
    server_type = "cx22"       # 2 vCPU, 4 GB RAM, 40 GB SSD, 20 TB egress/mo
    image       = "debian-12"
    location    = var.server_location
    ssh_keys    = [hcloud_ssh_key.pairwire_relay.id]

    user_data = local.cloud_init_rendered

    firewall_ids = [hcloud_firewall.pairwire_relay.id]

    # keep_disk = true preserves the disk on server REBUILD (e.g. OS reinstall via
    # terraform taint). It has no effect on server lifecycle — the server is only
    # destroyed by `terraform destroy` or manual deletion. This prevents accidental
    # data loss if the server is rebuilt to fix a cloud-init issue.
    keep_disk = true

    labels = {
      managed-by = "terraform"
      project    = "pairwire"
      role       = "relay"
    }
  }
  ```

- [ ] **Step 6.3 — Create `infra/terraform/dns.tf`** [agent]

  ```hcl
  # infra/terraform/dns.tf
  # Cloudflare DNS records for relay.pairwire.dev.
  # TTL 60s (low) so we can fail over quickly by updating the Hetzner server IP.

  resource "cloudflare_record" "relay_a" {
    zone_id = var.cloudflare_zone_id
    name    = "relay"
    type    = "A"
    content = hcloud_server.pairwire_relay.ipv4_address
    ttl     = 60
    proxied = false  # Must NOT be proxied — iroh uses raw TCP 443 WebSocket
                     # (end-to-end encrypted). Cloudflare's proxy would break
                     # the opaque WebSocket tunnel that carries QUIC packets.
    comment = "iroh-relay server IPv4 — managed by Terraform"
  }

  resource "cloudflare_record" "relay_aaaa" {
    zone_id = var.cloudflare_zone_id
    name    = "relay"
    type    = "AAAA"
    content = hcloud_server.pairwire_relay.ipv6_address
    ttl     = 60
    proxied = false
    comment = "iroh-relay server IPv6 — managed by Terraform"
  }
  ```

  **Important:** `proxied = false` is non-negotiable. Cloudflare's proxy terminates TLS and cannot forward the end-to-end encrypted WebSocket that iroh uses to tunnel QUIC. Setting `proxied = true` would break all relay connections silently.

- [ ] **Step 6.4 — Create `infra/terraform/outputs.tf`** [agent]

  ```hcl
  # infra/terraform/outputs.tf

  output "relay_url" {
    description = "The relay URL to set in PAIRWIRE_RELAY_URL or NetworkConfig.relay_url."
    value       = "https://relay.pairwire.dev"
    # This is the DNS name, not the raw IP, so TLS cert validation works correctly.
  }

  output "server_ip_v4" {
    description = "Hetzner server IPv4 address (also the A record target)."
    value       = hcloud_server.pairwire_relay.ipv4_address
  }

  output "server_ip_v6" {
    description = "Hetzner server IPv6 address (also the AAAA record target)."
    value       = hcloud_server.pairwire_relay.ipv6_address
  }

  output "ssh_connect" {
    description = "SSH command to connect to the relay server."
    value       = "ssh root@${hcloud_server.pairwire_relay.ipv4_address}"
    sensitive   = false
  }
  ```

- [ ] **Step 6.5 — Format and validate** [agent]

  ```bash
  cd /Users/bilal/Documents/personal/Github/rust-sharing/infra/terraform
  terraform fmt .
  # Expected: lists reformatted files (or nothing if already formatted)
  terraform init -backend=false
  # Expected: Terraform has been successfully initialized!
  terraform validate
  # Expected: Success! The configuration is valid.
  ```

- [ ] **Step 6.6 — Commit** [agent]

  ```bash
  git add infra/terraform/main.tf infra/terraform/dns.tf infra/terraform/outputs.tf \
          infra/terraform/cloud-init/relay.yaml
  git commit -m "feat(P4.5.1): add Terraform main/dns/outputs and cloud-init for iroh-relay 0.97.0"
  ```

---

### Task 7: Add justfile relay recipes [agent]

**Files:**
- Modify: `justfile`

#### Steps

- [ ] **Step 7.1 — Add relay recipes to justfile** [agent]

  Append to the end of `/Users/bilal/Documents/personal/Github/rust-sharing/justfile`:

  ```just
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
  ```

- [ ] **Step 7.2 — Verify justfile syntax** [agent]

  ```bash
  just --list 2>&1 | grep relay
  # Expected:
  # relay-apply     Apply the Terraform plan (provisions/updates the relay server)...
  # relay-destroy   Destroy all relay infrastructure...
  # relay-plan      Show what Terraform would change (safe, no mutations)
  ```

- [ ] **Step 7.3 — Commit** [agent]

  ```bash
  git add justfile
  git commit -m "feat(P4.5.1): add relay-plan/relay-apply/relay-destroy just recipes"
  ```

---

### Task 8: Run `terraform plan` and provision server [human]

This task requires the secrets from Task 4 to be in the environment.

**Files:** none (runs against remote Hetzner/Cloudflare APIs)

#### Steps

- [ ] **Step 8.1 — Export secrets and run plan** [human]

  ```bash
  export TF_VAR_hcloud_token="<your-hetzner-api-token>"
  export TF_VAR_cloudflare_token="<your-cloudflare-api-token>"
  export TF_VAR_cloudflare_zone_id="<your-zone-id>"
  export TF_VAR_admin_ip="$(curl -s https://checkip.amazonaws.com)/32"
  export TF_VAR_ssh_public_key="$(cat ~/.ssh/id_ed25519.pub)"

  just relay-plan
  ```

  **Verify expected plan output:**
  ```
  Plan: 5 to add, 0 to change, 0 to destroy.
  # Resources:
  #   hcloud_ssh_key.pairwire_relay
  #   hcloud_firewall.pairwire_relay
  #   hcloud_server.pairwire_relay
  #   cloudflare_record.relay_a
  #   cloudflare_record.relay_aaaa
  # Outputs: relay_url, server_ip_v4, server_ip_v6, ssh_connect
  ```

- [ ] **Step 8.2 — Apply** [human]

  ```bash
  just relay-apply
  # Type 'yes' when prompted.
  # Expected: Apply complete! Resources: 5 added, 0 changed, 0 destroyed.
  # (5 = hcloud_ssh_key + hcloud_firewall + hcloud_server + cloudflare_record x2)
  # Expected outputs:
  #   relay_url    = "https://relay.pairwire.dev"
  #   server_ip_v4 = "x.x.x.x"
  #   server_ip_v6 = "xxxx::xxxx"
  #   ssh_connect  = "ssh root@x.x.x.x"
  ```

- [ ] **Step 8.3 — Wait for cloud-init to complete** [human]

  First boot installs Rust + cargo install iroh-relay, which takes ~5-8 minutes.

  ```bash
  SSH_IP=$(cd infra/terraform && terraform output -raw server_ip_v4)
  ssh root@$SSH_IP 'cloud-init status --wait'
  # Expected: status: done
  # Then check the service:
  ssh root@$SSH_IP 'systemctl status iroh-relay.service'
  # Expected: Active: active (running)
  ssh root@$SSH_IP 'journalctl -u iroh-relay -n 50 --no-pager'
  # Look for: "relay started" or similar startup messages
  ```

- [ ] **Step 8.4 — Verify ACME certificate issued** [human]

  ACME TLS-ALPN-01 validation requires port 443 to be reachable with the correct DNS name. Wait ~60 seconds after service start for the cert to be issued.

  ```bash
  # Check TLS cert from outside (after DNS propagates):
  curl -v https://relay.pairwire.dev/relay/probe 2>&1 | grep "SSL certificate\|subject\|issuer\|expire\|HTTP/"
  # Expected: HTTP/1.1 200 OK (or 404 — the /relay/probe endpoint exists in iroh-relay)
  # Expected SSL: subject=CN=relay.pairwire.dev, issuer=Let's Encrypt
  ```

---

### Task 9: Manual relayed-session verification [human + agent]

This is the end-to-end validation that proves the relay is reachable and carries traffic.

**No new code files — verification only.**

#### Steps

- [ ] **Step 9.1 — Force relay-only mode and run a session** [human]

  On the host machine (build first):
  ```bash
  # Build with all features needed for a full session.
  # Use -p pairwire (the app crate) with explicit feature flags — not --workspace with
  # the `crate/feature` form, which is workspace-member-scoped and fragile.
  cargo build -p pairwire --features encode,capture,audio,gui,tray 2>&1 | tail -5
  # Expected: Compiling pairwire ... Finished dev [unoptimized + debuginfo] target(s) in Xs
  PAIRWIRE_RELAY_URL="https://relay.pairwire.dev" \
  RUST_LOG="iroh=debug,pairwire_transport=debug" \
    ./target/debug/pairwire host 2>&1 | tee /tmp/host.log &
  ```

  On the guest machine (or same machine in a second terminal):
  ```bash
  PAIRWIRE_RELAY_URL="https://relay.pairwire.dev" \
  RUST_LOG="iroh=debug,pairwire_transport=debug" \
    ./target/debug/pairwire join <invite-url> 2>&1 | tee /tmp/guest.log
  ```

- [ ] **Step 9.2 — Confirm traffic through the relay** [human]

  In the debug logs, look for:
  ```
  # On host or guest:
  pairwire_transport::transport: using merged relay map (custom + n0 prod) relay_url=https://relay.pairwire.dev total_relays=5
  # iroh debug showing relay path (not direct):
  iroh::...: relay addr used for connection
  ```

  On the relay server:
  ```bash
  ssh root@$SSH_IP 'journalctl -u iroh-relay -n 100 --no-pager | grep -i "accept\|relay\|client"'
  # Expected: lines showing connections being accepted and relayed
  ```

- [ ] **Step 9.3 — Check Prometheus metrics increment** [human]

  The metrics endpoint is on port 9090, accessible only from the server (not exposed in the firewall). SSH-tunnel to check:

  ```bash
  ssh -L 9090:localhost:9090 root@$SSH_IP -N &
  curl -s http://localhost:9090/metrics | grep -E "^relayserver_"
  # Expected output includes (metric names use the 'relayserver' prefix, NOT 'iroh_relay_'):
  # relayserver_accepts{...} 2              (counter — one per client connect)
  # relayserver_bytes_sent{...} <positive>  (bytes sent by relay to clients)
  # relayserver_bytes_recv{...} <positive>  (bytes received from clients)
  # relayserver_disconnects{...} <number>   (client disconnections)
  kill %1  # stop the tunnel
  ```

---

### Task 10: Grafana Cloud monitoring and 15 TB egress alert [human + agent]

iroh-relay exposes Prometheus metrics. We document the scrape config and alert rule here. Wiring them into Grafana Cloud requires a human (account + API key).

**Files:** no new files (documented inline; [human] to apply in Grafana Cloud UI)

#### Steps

- [ ] **Step 10.1 — Document Grafana Alloy scrape config** [agent]

  The metrics endpoint at `localhost:9090` is not publicly exposed (firewall blocks port 9090). The recommended approach is **Grafana Alloy** (formerly Grafana Agent) running on the relay server, which pushes metrics to Grafana Cloud over HTTPS. This avoids opening port 9090 to the public internet.

  Add the following block to `infra/terraform/cloud-init/relay.yaml` inside the `runcmd:` list, **after** the `systemctl start iroh-relay.service` line. This makes Alloy installation part of first-boot provisioning — no manual SSH required:

  ```yaml
    # Install Grafana Alloy (metrics push agent)
    - |
      curl -fsSL https://packages.grafana.com/gpg.key \
        | gpg --dearmor > /usr/share/keyrings/grafana.gpg
    - |
      echo "deb [signed-by=/usr/share/keyrings/grafana.gpg] \
        https://packages.grafana.com/alloy/deb stable main" \
        > /etc/apt/sources.list.d/grafana-alloy.list
    - apt-get update && apt-get install -y alloy
    - mkdir -p /etc/alloy
    - |
      cat > /etc/alloy/config.alloy <<'ALLOY'
      prometheus.scrape "iroh_relay" {
        targets = [{"__address__" = "localhost:9090"}]
        forward_to = [prometheus.remote_write.grafana_cloud.receiver]
        scrape_interval = "30s"
        job_name = "iroh-relay"
      }

      prometheus.remote_write "grafana_cloud" {
        endpoint {
          url = "https://prometheus-prod-01-eu-west-0.grafana.net/api/prom/push"
          basic_auth {
            username = env("GRAFANA_METRICS_USER_ID")
            password = env("GRAFANA_METRICS_API_KEY")
          }
        }
      }
      ALLOY
    - systemctl enable alloy && systemctl start alloy
  ```

  **Acceptance criteria for Alloy install:**
  ```bash
  ssh root@$SSH_IP 'systemctl is-active alloy'
  # Expected: active
  ssh root@$SSH_IP 'journalctl -u alloy -n 20 --no-pager | grep -i "error\|failed"'
  # Expected: no error lines (empty output is fine)
  ```

  **[human]:** The Alloy config above reads `GRAFANA_METRICS_USER_ID` and `GRAFANA_METRICS_API_KEY` from the environment. After creating your Grafana Cloud account and generating a Prometheus remote write key, add them to the server via:
  ```bash
  ssh root@$SSH_IP "echo 'GRAFANA_METRICS_USER_ID=<your-user-id>' >> /etc/environment"
  ssh root@$SSH_IP "echo 'GRAFANA_METRICS_API_KEY=<your-api-key>' >> /etc/environment"
  ssh root@$SSH_IP "systemctl restart alloy"
  ```
  Alternatively, bake these into a Terraform `hcloud_server_network` or a `systemd` drop-in — but do NOT commit the values to git.

  The Alloy config is embedded in the cloud-init block above. The final `/etc/alloy/config.alloy` on the server will read credentials from environment variables (set by the human step above).

  **[human]:** Create a Grafana Cloud account (free tier supports 10k active series). Generate a Prometheus remote write API key in your Grafana Cloud stack (Connections → Add new connection → Hosted Metrics → Generate a token). Use the `username` (numeric user ID) and `password` (API key) to set the environment variables on the relay server as shown above, then `systemctl restart alloy`.

- [ ] **Step 10.2 — Document 15 TB/mo egress alert rule** [human]

  In Grafana Cloud → Alerting → Alert rules, create a new alert:

  **Alert name:** `pairwire-relay-egress-15tb`

  **PromQL expression:**
  ```promql
  # iroh-relay (relayserver metrics prefix) exposes outbound bytes via relayserver_bytes_sent.
  # We alert on the sum of bytes_sent + bytes_recv as a proxy for total egress.
  # (There is no single "bytes_forwarded" counter — use the sum of both directions.)
  # This alert fires when the 30-day increase exceeds 15 TB (15 * 10^12 bytes).
  increase(relayserver_bytes_sent[30d]) + increase(relayserver_bytes_recv[30d]) > 15e12
  ```

  **Evaluation interval:** 1h

  **For duration:** 0s (alert immediately when threshold exceeded)

  **Annotations:**
  - Summary: `Relay egress >15 TB in rolling 30d window`
  - Description: `iroh-relay on relay.pairwire.dev has forwarded {{ $value | humanize1024 }}B in the last 30 days. Hetzner CX22 includes 20 TB/mo. Review traffic and consider restricting firewall or investigating abuse.`

  **Contact point:** Set up email / Slack notification channel.

  **Note on cost-cap mechanism:** iroh-relay has no per-node rate limiting (issue #2967). The firewall IP allowlist is the hard cap: removing `0.0.0.0/0` from the TCP 443 and UDP 7842 rules and replacing with specific IPs stops all public relay usage. This is a nuclear option — use the egress alert to decide before reaching Hetzner's 20 TB soft limit.

- [ ] **Step 10.3 — Commit monitoring documentation** [agent]

  ```bash
  # Use explicit paths — never git add -A after steps that may have set TF_VAR_* env vars
  # or generated local .tfvars files.
  git add infra/terraform/cloud-init/relay.yaml
  git commit -m "docs(P4.5.1): document Grafana Alloy scrape config and 15 TB egress alert rule"
  ```

---

### Task 11: Final validation — run `just all` [agent]

- [ ] **Step 11.1 — Run full workspace validation** [agent]

  ```bash
  just all 2>&1
  # Expected sequence:
  # cargo fmt --all -- --check   → no output (all files formatted)
  # cargo check --workspace       → no errors
  # cargo clippy --workspace -- -D warnings → no warnings
  # cargo test --workspace        → all tests pass
  ```

  If clippy warns about the `iroh_prod_defaults` import in `transport.rs` (unused import), ensure the import `use iroh::defaults::prod as iroh_prod_defaults;` is present and `iroh_prod_defaults::default_relay_map()` is called inside `bind()`. Both the module-level import and the test-module import must be present.

- [ ] **Step 11.2 — Verify `PAIRWIRE_RELAY_URL=""` explicitly disables custom relay** [agent]

  In a test shell:
  ```bash
  PAIRWIRE_RELAY_URL="" RUST_LOG="pairwire_transport=debug" \
    cargo run --bin pairwire -- host 2>&1 | head -10
  # Expected log line:
  # pairwire_transport::transport: relay_url explicitly disabled; using n0 public relays only
  ```

- [ ] **Step 11.3 — Verify default relay URL without env var** [agent]

  ```bash
  RUST_LOG="pairwire_transport=debug" \
    cargo run --bin pairwire -- host 2>&1 | head -10
  # Expected log line:
  # pairwire_transport::transport: using merged relay map (custom + n0 prod) \
  #   relay_url=https://relay.pairwire.dev total_relays=5
  ```

- [ ] **Step 11.4 — Final commit** [agent]

  ```bash
  git status
  # Review every untracked/modified file. Verify no .env, *.tfvars, or credentials appear.
  # Stage only known-safe files — do NOT use git add -A here.
  git add LEARNINGS.md
  git commit -m "feat(P4.5.1): Workstream B complete — relay IaC + client wiring + monitoring docs"
  ```

---

## Grafana Cloud Scrape Config (quick reference)

iroh-relay metrics exposed on `localhost:9090/metrics`. All metrics use the `relayserver_` prefix (from `#[metrics(name = "relayserver")]` in the iroh-relay source). Key metrics to dashboard:

| Metric | Type | Description |
|---|---|---|
| `relayserver_accepts` | Counter | Cumulative client connections accepted |
| `relayserver_bytes_sent` | Counter | Bytes sent by the relay to clients |
| `relayserver_bytes_recv` | Counter | Bytes received from clients |
| `relayserver_disconnects` | Counter | Client disconnections |

**Note:** There is no `relayserver_connected_clients` gauge, no `relayserver_bytes_forwarded_total`, and no `relayserver_got_disco_total`. Run `curl -s http://localhost:9090/metrics | grep '^relayserver'` after first boot to enumerate the full metric set for the installed version.

## Open questions / tradeoffs

**RelayMode::Custom replaces, not appends:** `Builder::relay_mode()` replaces any existing relay config (including the n0 default from `presets::N0`). We compensate by merging the custom map with `iroh_prod_defaults::default_relay_map()` (imported as `use iroh::defaults::prod as iroh_prod_defaults`) before passing to `relay_mode()`. The tradeoff: if the n0 prod relay map changes in a future iroh version, we pick up the change automatically (which is good). No manual sync needed.

**iroh-relay has no per-node rate limiting (issue #2967):** The relay is open by default. The cost-cap knob is the Hetzner firewall: modifying the TCP 443 / UDP 7842 `source_ips` from `0.0.0.0/0` to a specific CIDR stops public relay usage. For early beta with known testers this is acceptable; revisit when issue #2967 is closed.

**ACME contact email:** The cloud-init config hardcodes `ops@pairwire.dev` as the ACME contact. This address must exist and be monitored for Let's Encrypt expiry warnings (though auto-renewal handles the cert itself). Create an alias before going live.

**Cloudflare proxy must be off:** The `proxied = false` in `dns.tf` is non-negotiable. Cloudflare's proxy layer terminates TLS and cannot forward the opaque WebSocket tunnel that iroh uses to carry QUIC packets between peers. Enabling it would silently break all relayed connections.

**v0.96 hole-punch regression:** LEARNINGS.md (2026-03-30) documents that v0.96 had a regression where hole-punching was not re-triggered after path loss — sessions stay on relay forever. Verify iroh-relay 0.97.0 does not carry this regression by testing reconnect on real LTE (Task 9): drop the WiFi during a session, wait for reconnect, confirm iroh upgrades back to a direct path within 30s.
