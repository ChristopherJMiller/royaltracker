###############################################################################
# Cloudflare Tunnel (cloudflared) — inbound only.
#
# Flow:  browser -> Cloudflare edge -> Tunnel -> in-cluster cloudflared -> Service
#
# The tunnel is created with a remotely-managed config (config_src="cloudflare"),
# so the ingress rules below are the source of truth and the in-cluster
# cloudflared runs with just the token — no local config.yaml to keep in sync.
###############################################################################

# 32-byte random secret that both the tunnel and cloudflared share. Cloudflare
# wants it base64-encoded. Marked sensitive; stored only in TF state.
resource "random_password" "tunnel_secret" {
  length  = 48
  special = false
}

resource "cloudflare_zero_trust_tunnel_cloudflared" "public" {
  account_id    = var.account_id
  name          = var.tunnel_name
  config_src    = "cloudflare"
  tunnel_secret = base64encode(random_password.tunnel_secret.result)
}

resource "cloudflare_zero_trust_tunnel_cloudflared_config" "public" {
  account_id = var.account_id
  tunnel_id  = cloudflare_zero_trust_tunnel_cloudflared.public.id

  config = {
    ingress = [
      {
        hostname = var.public_hostname
        service  = var.origin_service_url
        origin_request = {
          # Preserve the public hostname so axum vhost/routing and any absolute
          # redirects behave as if hit directly.
          http_host_header = var.public_hostname
        }
      },
      # Catch-all is REQUIRED and must be last: anything not matched 404s.
      {
        service = "http_status:404"
      }
    ]
  }
}

# Connector token the in-cluster cloudflared authenticates with. Surfaced as a
# sensitive output; the actual k8s Secret is created later in luma-homeops.
data "cloudflare_zero_trust_tunnel_cloudflared_token" "public" {
  account_id = var.account_id
  tunnel_id  = cloudflare_zero_trust_tunnel_cloudflared.public.id
}
