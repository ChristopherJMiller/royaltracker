# royaltracker

A personal Telegram bot for tracking Royal Caribbean and Celebrity Cruise Planner add-on prices. Logs in as you each day, fetches your personalized prices for the items you're watching, and Telegrams you when something drops.

The point: RCG shows different prices to different people (loyalty tier, casino offers, age, cabin class). Public price trackers only see the logged-out catalog. This one logs in.

Built for two users. Not a service.

## Running it

```sh
nix develop
cp config.example.toml config.toml   # fill in bot_token and encryption_key_b64
nix run .#dev                        # bot + a cloudflared quick-tunnel
```

Open the bot in Telegram, `/start`, link your RCG account in the Mini App, refresh the catalog, pick what to watch. The scraper runs once per day in production; in dev you can fire it on demand:

```sh
cargo run -p royaltracker-scraper --no-default-features --features sqlite
```

Two binaries, one image:
- **`royaltracker-bot`** — long-poll Telegram dispatcher + Mini App HTTP server, runs as a Deployment
- **`royaltracker-scraper`** — one-shot, runs as a CronJob, fetches today's prices and notifies on drops ≥$1 and ≥1%

## Configuration

`config.toml` (gitignored) or env vars with prefix `ROYALTRACKER_` (nested keys via `__`):

```toml
database_url       = "sqlite:dev.db?mode=rwc"   # or postgres://...
encryption_key_b64 = "<32 bytes base64>"        # cargo run -p royaltracker-crypto --example gen-key
rcg_basic_auth_b64 = "<from RCG JS bundle>"     # see config.example.toml

[telegram]
bot_token = "<from @BotFather>"

[web]
public_url = "https://example.com"
bind_addr  = "0.0.0.0:8080"
```

User passwords are encrypted at rest with ChaCha20-Poly1305 using `encryption_key_b64`; lose the key and they're unrecoverable.

## Deployment

Helm chart at `charts/royaltracker/` deploys both binaries from one image, sharing a Postgres database (or local SQLite PVC if you prefer). Assumes you bring your own ingress controller, cert issuer, and secret-management strategy.

CI publishes images to GHCR on every push to `main` (`.github/workflows/image.yml`).

## Risk and legality

This is a personal-use bot using your own credentials — same posture as the open-source `jdeath/CheckRoyalCaribbeanPrice` that the Royal Caribbean Blog itself recommends. RCG's ToU technically forbids automated access; CFAA doesn't apply post-Van Buren (you're authorized — they're your credentials). Mitigations: residential IP, ≤2 logins per user per day, drop-only alerts, no public redistribution of pricing data, stop on any directed cease-and-desist.

## License

MIT.
