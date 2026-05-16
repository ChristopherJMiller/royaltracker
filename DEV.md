# Dev workflow

Two terminals:

## Terminal 1 — the bot + Mini App HTTP server

```sh
nix develop -c cargo run -q -p royaltracker-bot --no-default-features --features sqlite
```

Listens on `0.0.0.0:8080` (configured in `[web].bind_addr`).

## Terminal 2 — public HTTPS tunnel for the Mini App

Telegram refuses to load a Mini App from an HTTP URL or from one without a valid cert.
For development we use Cloudflare's free quick-tunnel:

```sh
cloudflared tunnel --url http://localhost:8080
```

Copy the printed `https://*.trycloudflare.com` URL into `[web].public_url` in
`config.toml`, then **restart the bot** (it reads the URL at startup to set the
chat menu button + inline keyboard `web_app` button).

The bot restart is fast (~5s) because all deps are cached.

## In Telegram

1. Open the bot chat.
2. Type `/start` → tap **🛳️ Open dashboard** in the reply.
3. Alternatively, the bottom-left menu button is now a permanent "Dashboard" launcher.
