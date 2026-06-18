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
  server_type = "cx22" # 2 vCPU, 4 GB RAM, 40 GB SSD, 20 TB egress/mo
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
