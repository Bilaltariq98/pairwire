# infra/terraform/dns.tf
# Cloudflare DNS records for relay.pairwire.dev.
# TTL 60s (low) so we can fail over quickly by updating the Hetzner server IP.

resource "cloudflare_record" "relay_a" {
  zone_id = var.cloudflare_zone_id
  name    = "relay"
  type    = "A"
  content = hcloud_server.pairwire_relay.ipv4_address
  ttl     = 60
  proxied = false # Must NOT be proxied — iroh uses raw TCP 443 WebSocket
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
