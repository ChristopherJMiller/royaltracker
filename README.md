# royaltracker

Personal Telegram bot that watches Cruise Planner add-on prices on Royal Caribbean and Celebrity for your own bookings and pings you when they drop.

Single Rust workspace, two binaries (`cruise-bot` long-poll Deployment + `cruise-scraper` CronJob), SQLite for dev / Postgres for prod, packaged via a Nix flake.

## Quick start

```sh
nix develop                                          # rust toolchain + sqlx-cli + sops + kubectl
cp config.example.toml config.toml                   # fill in secrets
cargo check --no-default-features --features sqlite -p cruise-scraper
cargo run  --no-default-features --features sqlite -p cruise-scraper
```

For Postgres dev:

```sh
export DATABASE_URL=postgres://...
cargo run --no-default-features --features postgres -p cruise-scraper
```

## Build the OCI image

```sh
nix build .#cruise-bot-image
docker load < result
nix build .#cruise-scraper-image
```

## Status

Scaffolded per `/home/chris/.claude/plans/form-a-plan-for-binary-pearl.md`. Phase 0 ground-truth against jdeath's Python reference is the next step before relying on the Rust client.
