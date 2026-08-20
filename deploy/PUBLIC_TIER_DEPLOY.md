# Deploying the no-login public tier

The public tier ships inside the existing `royaltracker-bot` pod (served at `/p`
and `/public/*`). It's **disabled by default** — enabling it is a config +
secrets change in `luma-homeops`, gated behind `public.enabled`.

Order matters: **(1) merge to main so the image builds → (2) add secrets →
(3) add the pg role → (4) provision Cloudflare → (5) flip `public.enabled`.**

## 1. Land the image
Merge the `public-tier` PR to `main`. `.github/workflows/image.yml` builds
`ghcr.io/christopherjmiller/royaltracker-{bot,scraper}:<sha>`. Note the SHA.

## 2. Secrets (`cluster/royaltracker/config.secret.yaml`, then `./sign.sh`)
Add these keys to the `royaltracker-config` secret:

| key | how to get it |
|-----|----------------|
| `vapid_private_key_b64` | `nix develop -c cargo run -q -p royaltracker-notify --example gen_vapid` |
| `vapid_public_key_b64`  | same command (printed alongside) |
| `device_cookie_key_b64` | `head -c 32 /dev/urandom \| base64` (any ≥16-byte key) |
| `turnstile_site_key`    | Cloudflare Turnstile widget (OpenTofu output, step 4) |
| `turnstile_secret`      | Cloudflare Turnstile widget secret (step 4) |
| `public_database_url`   | *(optional, step 3)* `postgresql://royaltracker_public:<pw>@acid-royaltracker.royaltracker:5432/royaltracker` |

Re-seal with `./sign.sh` and commit the sealed output. (Do all `luma-homeops`
commits inside `nix-shell shell.nix --run 'git commit …'` so the pre-commit hook
runs — never `--no-verify`.)

## 3. Least-privilege DB role (optional but recommended)
In `cluster/royaltracker/psql.yaml`, add the role so the operator creates it and
a secret for it:

```yaml
  users:
    royaltracker:
      - superuser
      - createdb
    royaltracker_public: []      # login role, no attributes
```

Argo syncs → Zalando creates role `royaltracker_public` + secret
`royaltracker-public.acid-royaltracker.credentials.postgresql.acid.zalan.do`.
Migration `0011_public_role_grants.sql` (runs as the `royaltracker` superuser on
bot/scraper boot) then GRANTs it the public tables and REVOKEs the authed ones —
verified: it gets `(none)` on `users`/`bookings`. Put its `public_database_url`
(username `royaltracker_public`, password from that secret) into the config
secret and set `public.useLeastPrivRole: true`. Skip this and the public router
just uses the main superuser pool (fine for a first launch).

## 4. Cloudflare (OpenTofu, `deploy/cloudflare/`)
Once the CF account/API token/zone exist:
```
cd deploy/cloudflare
cp terraform.tfvars.example terraform.tfvars   # fill in account_id, zone_id, hostname, token
tofu init && tofu apply
```
Outputs: the **tunnel token** (→ seal as `royaltracker-cloudflared` secret, key
`token`) and the **Turnstile site key + secret** (→ config secret, step 2). Use a
remote/encrypted TF state backend — state holds those secrets. The tunnel is
**inbound only**; the scraper's cruise-line egress stays on the home IP.

## 5. Flip it on (`cluster/applications/royaltracker-helm.yaml`)
Bump `image.tag` to the step-1 SHA and add:
```yaml
        public:
          enabled: true
          vapidSubject: "mailto:you@example.com"
          tunnelHostname: "prices.chrismiller.xyz"
          useLeastPrivRole: true          # if you did step 3
          cloudflared:
            enabled: true                 # if you did step 4
```
Argo rolls the bot pod (Recreate). Read-only lookup works immediately; subscribe
lights up once Turnstile + VAPID secrets are present.

### Sanity checks
- `GET https://<host>/public/ships` → 40 ships.
- `GET https://<host>/public/config` → `subscribe_enabled: true`, a turnstile site key, a VAPID public key.
- `GET https://<host>/p` → the lookup PWA.
- The daily scraper CronJob logs `public sweep complete` and refreshes tracked sailings.

## Notes / deferred
- iOS web push requires Add-to-Home-Screen first; the UI says so, and points at the Telegram bot as the fallback.
- Snapshot retention / cleanup of tracked products that no one subscribes to is not yet implemented — revisit before a broad launch (casual lookups seed sweepable rows).
- The separate least-privilege `royaltracker-web` binary + its own Deployment (vs. running inside the bot pod) remains a future hardening step.
