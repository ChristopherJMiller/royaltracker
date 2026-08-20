###############################################################################
# Input variables. Secrets (api token) come from TF_VAR_* env; everything else
# has a sensible default or is set in terraform.tfvars (see the .example).
###############################################################################

# ---- Credentials (never committed; pass via TF_VAR_cloudflare_api_token) ----

variable "cloudflare_api_token" {
  description = <<-EOT
    Cloudflare API token with permissions for the target account+zone:
      Account : Cloudflare Tunnel:Edit, Turnstile:Edit
      Zone    : DNS:Edit, Zone Settings:Edit, Zone WAF:Edit (Rulesets), Cache Rules:Edit
    Supply out of band: `export TF_VAR_cloudflare_api_token=...` (or CLOUDFLARE_API_TOKEN).
  EOT
  type        = string
  sensitive   = true
}

# ---- Account / zone identity ----

variable "account_id" {
  description = "Cloudflare account ID that owns the tunnel + Turnstile widget."
  type        = string
}

variable "zone_id" {
  description = "Cloudflare zone ID for the domain hosting the public hostname (e.g. chrismiller.xyz)."
  type        = string
}

variable "zone_name" {
  description = "Apex/zone name, used only for documentation/tagging (e.g. chrismiller.xyz)."
  type        = string
  default     = "chrismiller.xyz"
}

# ---- Public hostname + origin ----

variable "public_hostname" {
  description = <<-EOT
    Dedicated public hostname for the no-login tier, distinct from the authed
    Telegram Mini App host (rccl-tracker.chrismiller.xyz). e.g. prices.chrismiller.xyz.
  EOT
  type    = string
  default = "prices.chrismiller.xyz"
}

variable "origin_service_url" {
  description = <<-EOT
    URL the in-cluster cloudflared connects to as the tunnel origin. Points at
    the royaltracker web Service. Chart Service is `royaltracker-bot` on port 80
    in namespace `royaltracker`, so the cluster-internal DNS name is:
      http://royaltracker-bot.royaltracker.svc.cluster.local:80
    Override if the public tier gets its own Service/port.
  EOT
  type    = string
  default = "http://royaltracker-bot.royaltracker.svc.cluster.local:80"
}

# ---- Tunnel ----

variable "tunnel_name" {
  description = "Name of the Cloudflare Tunnel (shown in the Zero Trust dashboard)."
  type        = string
  default     = "royaltracker-public"
}

# ---- Turnstile ----

variable "turnstile_widget_name" {
  description = "Human-readable name for the Turnstile widget."
  type        = string
  default     = "royaltracker-public-signup"
}

variable "turnstile_domains" {
  description = <<-EOT
    Domains the Turnstile widget is valid for. Include the public hostname and,
    for local dev, `localhost`. Cloudflare treats subdomains as covered by the
    parent, but list explicitly to be safe.
  EOT
  type    = list(string)
  default = ["prices.chrismiller.xyz", "localhost"]
}

# ---- Cache + rate limit + WAF tuning ----

variable "public_api_path_prefix" {
  description = "URL path prefix for cacheable public read endpoints (price/catalog lookups)."
  type        = string
  default     = "/api/public/"
}

variable "signup_path" {
  description = "Exact URL path of the public signup mutation, guarded by a managed challenge."
  type        = string
  default     = "/api/public/subscribe"
}

variable "edge_cache_ttl_seconds" {
  description = "Edge cache TTL for public price responses. Prices refresh daily, so hours is fine."
  type        = number
  default     = 3600
}

variable "browser_cache_ttl_seconds" {
  description = "Browser cache TTL for public price responses."
  type        = number
  default     = 300
}

variable "ratelimit_requests_per_minute" {
  description = "Allowed requests per IP per 60s window on the public hostname before blocking."
  type        = number
  default     = 120
}

variable "ratelimit_block_seconds" {
  description = "How long (seconds) to block an IP that trips the rate limit. Min 10 on most plans."
  type        = number
  default     = 60
}
