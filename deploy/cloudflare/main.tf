###############################################################################
# royaltracker — Cloudflare edge for the PUBLIC (no-login) tier
#
# This module is APPLY-LATER. It is written against a Cloudflare account the
# user is still provisioning. Do NOT `tofu apply` until the account, an API
# token, and the zone exist. No secrets live in these files — everything
# sensitive comes in via TF_VAR_* environment variables (see README.md).
#
# Scope of this module (INBOUND edge only):
#   * A named Cloudflare Tunnel + its remotely-managed ingress config that
#     routes a dedicated public hostname to the in-cluster web Service.
#   * The proxied DNS CNAME that points the hostname at the tunnel.
#   * A Turnstile widget for the public signup page.
#   * Zone settings + a cache rule so public price responses cache at the edge,
#     plus a rate-limit rule and a managed-challenge WAF rule on signup.
#
# OUT OF SCOPE (handled later in luma-homeops, NOT here):
#   * The in-cluster `cloudflared` Deployment and its credentials Secret
#     (fed from `tunnel_token` output).
#   * The web app's Turnstile *secret* wiring (fed from `turnstile_secret`).
#
# The SCRAPER is unaffected: it keeps egressing from the home IP. This tunnel
# is strictly inbound (browser -> Cloudflare -> tunnel -> cluster). See README.
###############################################################################

terraform {
  required_version = ">= 1.6.0"

  required_providers {
    cloudflare = {
      source  = "cloudflare/cloudflare"
      # v5 schemas are API-generated and differ substantially from v4. Pin to a
      # recent v5 minor; run `tofu init -upgrade` if a resource attribute is
      # rejected on first apply.
      version = "~> 5.8"
    }
    random = {
      source  = "hashicorp/random"
      version = "~> 3.6"
    }
  }
}

# API token supplied out of band as TF_VAR_cloudflare_api_token (or the standard
# CLOUDFLARE_API_TOKEN env var, which the provider also reads). Never committed.
provider "cloudflare" {
  api_token = var.cloudflare_api_token
}

###############################################################################
# Zone settings — keep public traffic on modern TLS and force HTTPS.
# One resource per setting in provider v5.
###############################################################################

resource "cloudflare_zone_setting" "always_use_https" {
  zone_id    = var.zone_id
  setting_id = "always_use_https"
  value      = "on"
}

resource "cloudflare_zone_setting" "min_tls_version" {
  zone_id    = var.zone_id
  setting_id = "min_tls_version"
  value      = "1.2"
}

resource "cloudflare_zone_setting" "tls_1_3" {
  zone_id    = var.zone_id
  setting_id = "tls_1_3"
  value      = "on"
}

resource "cloudflare_zone_setting" "brotli" {
  zone_id    = var.zone_id
  setting_id = "brotli"
  value      = "on"
}

###############################################################################
# Edge cache rule — public price responses are sailing-level and identical for
# every viewer, so cache them at the edge and stop them multiplying origin
# hits. Scoped to the public read paths only; signup/manage stay uncached.
###############################################################################

resource "cloudflare_ruleset" "public_cache" {
  zone_id = var.zone_id
  name    = "royaltracker-public-cache"
  kind    = "zone"
  phase   = "http_request_cache_settings"

  rules = [
    {
      ref         = "cache_public_prices"
      description = "Edge-cache public price/catalog reads"
      enabled     = true
      expression  = "(http.host eq \"${var.public_hostname}\" and starts_with(http.request.uri.path, \"${var.public_api_path_prefix}\") and http.request.method eq \"GET\")"
      action      = "set_cache_settings"
      action_parameters = {
        cache = true
        edge_ttl = {
          mode    = "override_origin"
          default = var.edge_cache_ttl_seconds
        }
        browser_ttl = {
          mode    = "override_origin"
          default = var.browser_cache_ttl_seconds
        }
        # Query string is part of the sailing key (brand/ship/sailDate), so it
        # must vary the cached object.
        cache_key = {
          cache_by_device_type = false
          ignore_query_strings_order = false
        }
      }
    }
  ]
}

###############################################################################
# Rate limit — cap per-IP request rate on the public hostname so a scraper of
# our scraper can't hammer the origin (and, transitively, RCCL).
###############################################################################

resource "cloudflare_ruleset" "public_ratelimit" {
  zone_id = var.zone_id
  name    = "royaltracker-public-ratelimit"
  kind    = "zone"
  phase   = "http_ratelimit"

  rules = [
    {
      ref         = "ratelimit_public"
      description = "Per-IP rate limit on the public hostname"
      enabled     = true
      expression  = "(http.host eq \"${var.public_hostname}\")"
      action      = "block"
      ratelimit = {
        characteristics     = ["ip.src", "cf.colo.id"]
        period              = 60
        requests_per_period = var.ratelimit_requests_per_minute
        mitigation_timeout  = var.ratelimit_block_seconds
      }
    }
  ]
}

###############################################################################
# WAF custom rule — force a managed challenge on the signup mutation. Turnstile
# already guards the form in-app; this is defense in depth at the edge against
# bots POSTing straight to the endpoint.
###############################################################################

resource "cloudflare_ruleset" "public_waf" {
  zone_id = var.zone_id
  name    = "royaltracker-public-waf"
  kind    = "zone"
  phase   = "http_request_firewall_custom"

  rules = [
    {
      ref         = "challenge_signup"
      description = "Managed challenge on the public signup endpoint"
      enabled     = true
      expression  = "(http.host eq \"${var.public_hostname}\" and http.request.uri.path eq \"${var.signup_path}\" and http.request.method eq \"POST\")"
      action      = "managed_challenge"
    }
  ]
}
