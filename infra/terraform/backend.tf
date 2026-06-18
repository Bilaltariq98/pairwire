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
  cloud {
    organization = "pairwire"

    workspaces {
      name = "pairwire-relay"
    }
  }
}

# Option B (advanced): Cloudflare R2 — comment the above backend block and uncomment this:
#
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
