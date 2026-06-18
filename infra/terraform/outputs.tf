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
