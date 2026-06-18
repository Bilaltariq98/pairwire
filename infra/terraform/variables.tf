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
    Your current egress IP address, in CIDR notation.
    IPv4 example: 1.2.3.4/32  (find with: curl -s https://checkip.amazonaws.com && echo /32)
    IPv6 example: 2001:db8::1/128
    This is the only IP allowed to SSH into the relay server.
  EOT
  type        = string

  validation {
    # cidrnetmask() only accepts IPv4; cidrsubnet() accepts both IPv4 and IPv6.
    # Using both ensures operators with IPv6-only egress can also apply firewall rules.
    condition     = can(cidrnetmask(var.admin_ip)) || can(cidrsubnet(var.admin_ip, 0, 0))
    error_message = "admin_ip must be a valid IPv4 or IPv6 CIDR block (e.g. 1.2.3.4/32 or 2001:db8::1/128)."
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
