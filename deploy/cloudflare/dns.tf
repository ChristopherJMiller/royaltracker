###############################################################################
# DNS — proxied CNAME pointing the public hostname at the tunnel.
#
# `<tunnel-id>.cfargotunnel.com` is the magic tunnel target. It MUST be proxied
# (orange cloud) — that's what routes traffic through Cloudflare into the
# tunnel and hides the origin IP.
###############################################################################

resource "cloudflare_dns_record" "public" {
  zone_id = var.zone_id
  name    = var.public_hostname
  type    = "CNAME"
  content = "${cloudflare_zero_trust_tunnel_cloudflared.public.id}.cfargotunnel.com"
  proxied = true
  ttl     = 1 # 1 = "automatic"; required when proxied.
  comment = "royaltracker public tier — routed via Cloudflare Tunnel (managed by OpenTofu)"
}
