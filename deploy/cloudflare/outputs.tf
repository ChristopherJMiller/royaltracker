###############################################################################
# Outputs. The two sensitive values feed secrets created LATER in luma-homeops
# (out of scope here). Read them post-apply with:
#   tofu output -raw tunnel_token
#   tofu output -raw turnstile_secret
###############################################################################

output "tunnel_id" {
  description = "Cloudflare Tunnel ID (also the CNAME target prefix: <id>.cfargotunnel.com)."
  value       = cloudflare_zero_trust_tunnel_cloudflared.public.id
}

output "tunnel_name" {
  description = "Cloudflare Tunnel name."
  value       = cloudflare_zero_trust_tunnel_cloudflared.public.name
}

output "tunnel_token" {
  description = <<-EOT
    Connector token for the in-cluster cloudflared. In luma-homeops, put this in
    a k8s Secret (e.g. `royaltracker-cloudflared` key `token`) and run cloudflared
    with `tunnel run --token <token>`. SENSITIVE — do not commit or log.
  EOT
  value       = data.cloudflare_zero_trust_tunnel_cloudflared_token.public.token
  sensitive   = true
}

output "public_hostname" {
  description = "The public hostname now served through the tunnel."
  value       = var.public_hostname
}

output "public_cname_target" {
  description = "The proxied CNAME target for the public hostname."
  value       = cloudflare_dns_record.public.content
}

output "turnstile_site_key" {
  description = <<-EOT
    Turnstile SITE key (public). Embed in the signup page's widget. Not secret,
    but surfaced here so the web app config can reference one canonical value.
  EOT
  value       = cloudflare_turnstile_widget.signup.id
}

output "turnstile_secret" {
  description = <<-EOT
    Turnstile SECRET key for server-side siteverify. In luma-homeops, put this in
    the web app's config Secret (e.g. `royaltracker-config` key `turnstile_secret`).
    SENSITIVE — do not commit or log.
  EOT
  value       = cloudflare_turnstile_widget.signup.secret
  sensitive   = true
}
