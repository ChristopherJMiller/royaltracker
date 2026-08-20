# royaltracker — Cloudflare edge (OpenTofu)

Apply-**later** IaC for the Cloudflare edge in front of royaltracker's **public,
no-login tier**. Nothing here is applied yet: the Cloudflare account is still
being provisioned. When it exists, follow the steps below.

This module provisions, all **inbound**:

| Resource | Purpose |
| --- | --- |
| `cloudflare_zero_trust_tunnel_cloudflared` + `_config` | Named Cloudflare Tunnel routing the public hostname to the in-cluster web Service |
| `cloudflare_dns_record` | Proxied CNAME `prices.<zone>` → `<tunnel-id>.cfargotunnel.com` |
| `cloudflare_turnstile_widget` | Managed-mode bot check for the public signup page |
| `cloudflare_ruleset` (cache) | Edge-cache public price reads so they don't multiply origin hits |
| `cloudflare_ruleset` (ratelimit) | Per-IP rate limit on the public hostname |
| `cloudflare_ruleset` (WAF) | Managed challenge on the signup POST |
| `cloudflare_zone_setting` ×4 | Force HTTPS, TLS ≥1.2, TLS 1.3, Brotli |

## Scraper egress is NOT touched

This tunnel is **inbound only**: browser → Cloudflare edge → Tunnel →
in-cluster `cloudflared` → `royaltracker-bot` Service. The **scraper keeps
egressing from the home IP** — do **not** move scraping behind Cloudflare.
Cloudflare egress IPs plus no TLS-fingerprint control would get Akamai-blocked
by RCCL. Only the public web app is fronted here.

## Prerequisites

1. A Cloudflare account and a zone (e.g. `chrismiller.xyz`) already added.
2. An **API token** (My Profile → API Tokens → Create Token) with:
   - **Account** → Cloudflare Tunnel: *Edit*, Turnstile: *Edit*
   - **Zone** → DNS: *Edit*, Zone Settings: *Edit*, Zone WAF (Rulesets): *Edit*,
     Cache Rules: *Edit*
   scoped to the target account + zone.
3. `account_id` and `zone_id` (dashboard → zone overview, right rail).
4. OpenTofu ≥ 1.6 (`tofu`) — or Terraform ≥ 1.6.

## Apply

```sh
cd deploy/cloudflare

# 1. Secrets via env — never in tfvars/state-in-git.
export TF_VAR_cloudflare_api_token="<token>"

# 2. Non-secret vars.
cp terraform.tfvars.example terraform.tfvars
$EDITOR terraform.tfvars          # set account_id, zone_id, hostname, ...

# 3. Init + review + apply.
tofu init
tofu plan
tofu apply
```

If a resource attribute is rejected on first init (v5 schemas are
API-generated), run `tofu init -upgrade` to pull the newest provider patch.

## Hand-off to the cluster (done later in luma-homeops, NOT here)

Two output values feed Kubernetes Secrets that live in
`/home/chris/Repos/luma-homeops` — this module deliberately does **not** create
them:

```sh
# Connector token for the in-cluster cloudflared Deployment.
tofu output -raw tunnel_token
#   -> k8s Secret, e.g.  royaltracker-cloudflared / token
#      cloudflared runs:  tunnel run --token <token>   (no local config; config is remote)

# Turnstile secret for server-side siteverify in the web app.
tofu output -raw turnstile_secret
#   -> add to the web app config Secret, e.g.  royaltracker-config / turnstile_secret
```

The **Turnstile site key** (`tofu output turnstile_site_key`) is public and gets
embedded in the signup page markup.

Because the tunnel uses a **remotely-managed config** (`config_src=cloudflare`),
the in-cluster `cloudflared` needs only the token — the ingress rules are the
source of truth in `tunnel.tf`, not a local `config.yaml`.

## Origin target

`origin_service_url` defaults to the chart's Service:

```
http://royaltracker-bot.royaltracker.svc.cluster.local:80
```

(`royaltracker-bot` Service, port 80, namespace `royaltracker` — see
`charts/royaltracker/templates/service.yaml`). The bot process also hosts the
axum web server, so today the public tier shares that Service. If the public
tier later gets its own Service/port, override `origin_service_url`.

## Notes / caveats

- **State contains secrets.** `random_password.tunnel_secret`, the tunnel token,
  and the Turnstile secret land in TF state. Use an encrypted/remote backend or
  keep state out of git. No backend is configured here — add one before apply.
- **Two hostnames.** The authed Telegram Mini App stays on
  `rccl-tracker.chrismiller.xyz` (nginx Ingress, external-dns, cert-manager,
  unchanged). The public tier gets a **separate** hostname (`prices.<zone>`)
  fronted only by Cloudflare. Keep them distinct so the walled-off public tier
  never shares the authed host.
- **Edge cache scope.** The cache rule only matches `GET` under
  `public_api_path_prefix` (`/api/public/`). Signup/manage endpoints are never
  edge-cached. Tune `public_api_path_prefix` to match the real router paths once
  the public web handlers land.
- **Rate limit plan floor.** `ratelimit_block_seconds` has a plan-dependent
  minimum (often 10s); the default 60s is safe.
