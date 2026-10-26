# Deploying Princess support

Princess support ships in the existing `royaltracker-bot` and
`royaltracker-scraper` images. It adds **no new secrets or config keys**: the
Princess guest client reuses the `pacing` settings, and accounts are linked by
users through `/register` or the Mini App. Deploying it is the usual two-repo
flow, with no `luma-homeops` changes beyond the image tag.

Order: **(1) test the migrations on a scratch Postgres → (2) run the Princess
login smoke test once → (3) merge to main so the image builds → (4) bump the tag.**

## What the migrations do
Both run automatically when the bot or scraper boots, as the `royaltracker`
superuser.

- **0012_brand_princess**: adds `'princess'` to the `brand_kind` enum. It is in
  its own migration because Postgres can't use a new enum value in the same
  transaction that adds it.
- **0013_cruise_accounts**:
  - creates `cruise_accounts`, one login per (user, cruise line), and copies
    every existing user's RCG credentials into an `rcg` row;
  - makes `users.rcg_username`, `rcg_password_ct`, `rcg_password_nonce` and
    `brand_pref` nullable (the columns stay but are no longer written);
  - adds `voyage_code` and `currency` to `bookings` and `sailings`;
  - changes the `sailings` unique key to
    `(brand, ship_code, sail_date, voyage_code)`;
  - empties `deck_plans` (a lazily refilled cache) and keys it by
    `(brand, ship_code, deck)` with a `NOT NULL brand`;
  - links pre-existing bookings to a sailing row.

## Rollback: only before anything new is written
The migrations are one-way. Rolling the image back to the pre-Princess tag is
safe **only before** the new image has handled any new registration, written
any Princess data, or served any public lookup. On the migrated schema the old
image breaks like this:

- **`NULL users.rcg_username`**: users created by the new image keep their
  credentials in `cruise_accounts`, not `users`, and the old image fails to
  read those rows.
- **`princess` brand rows** in `bookings` or `sailings`: the old image can't
  decode the brand.
- **The `sailings` unique key** is now `(brand, ship_code, sail_date, voyage_code)`.
  The old `upsert_sailing` targets `(brand, ship_code, sail_date)` and fails, so
  public lookups and subscribe break.
- **`deck_plans.brand`** is `NOT NULL` and part of the key. The old deck-plan
  insert lacks it and fails, so new deck plans aren't cached.

The last two hold as soon as the migrations have run, so even inside the safe
window a rolled-back image is degraded. Once the window has passed, **roll
forward** with a fix. Don't try to revert the migrations.

A plain tag rollback won't even boot once the new image has run. The old
image's sqlx migrator refuses a database that has applied migrations it
doesn't know, so it exits with a `db migrate` error (`VersionMissing(N)`). This
holds for **every** later migration too, including additive ones like 0014. To
roll back anyway, set aside the bookkeeping rows newer than the target image's
last migration (the schema changes stay). `N` is that last migration: 13 for
`ab2e60a` (the first Princess image), 9 for `fa861d2` (before the public tier).

```sql
CREATE TABLE _sqlx_migrations_princess AS SELECT * FROM _sqlx_migrations WHERE version > N;
DELETE FROM _sqlx_migrations WHERE version > N;
```

0014 (`bookings.deck_name`, `cabin_category`) only adds nullable columns the
older images never read, so rolling back to `ab2e60a` costs nothing beyond this
bookkeeping step.

Put them back **before** rolling forward again. Otherwise the set-aside
migrations re-run, and 0013 fails on the existing `cruise_accounts` table:

```sql
INSERT INTO _sqlx_migrations SELECT * FROM _sqlx_migrations_princess;
DROP TABLE _sqlx_migrations_princess;
```

Registrations made while the old image runs only update `users`, not
`cruise_accounts`, so those users have to `/register` again after rolling
forward. This escape hatch hasn't been exercised end to end, so rehearse it on
a scratch copy first.

## 1. Test the migrations on a scratch Postgres 16
`pg_migrate` needs an **empty** database, and it skips (passes vacuously)
unless `PG_TEST_URL` is set. This throwaway server listens on TCP localhost
only, with unix sockets off, so a long temp path can't hit the socket-path
length limit.

```bash
export D=$(mktemp -d)
nix-shell -p postgresql_16 --run "initdb -D $D -U postgres >/dev/null && pg_ctl -D $D -o '-c listen_addresses=localhost -c unix_socket_directories= -p 55432' -l $D/log start && createdb -h localhost -p 55432 -U postgres rt"
PG_TEST_URL="postgres://postgres@localhost:55432/rt" \
  nix develop -c cargo test -p royaltracker-storage --features postgres --test pg_migrate
nix-shell -p postgresql_16 --run "pg_ctl -D $D stop" && rm -rf "$D"
```

Expect `1 passed`. If it fails on a constraint name, compare against `\d sailings`
on the scratch database.

## 2. Princess login smoke test (run once, by the account owner)
This makes a real login against Princess, so run it **once** and don't loop
it: the per-IP auth rate limit is unknown. It skips unless both variables are
set. Use your own account, and read the password without echoing it, so it
stays out of shell history:

```bash
read -r PRINCESS_LOGIN; read -rs PRINCESS_PASSWORD; export PRINCESS_LOGIN PRINCESS_PASSWORD
nix develop -c cargo test -p royaltracker-api --test princess_login_smoke -- --nocapture
unset PRINCESS_LOGIN PRINCESS_PASSWORD
```

Equivalent one-liner: `PRINCESS_LOGIN=… PRINCESS_PASSWORD=… nix develop -c cargo test -p royaltracker-api --test princess_login_smoke -- --nocapture`.

It should print `ok: booking … ship … voyage …`. The output includes your
booking id, so don't paste it anywhere public. If it fails, the error carries
the HTTP status and a short body snippet (with token fields removed).

## 3. Land the image
Merge the `princess-support` PR to `main`. `.github/workflows/image.yml` builds
`ghcr.io/christopherjmiller/royaltracker-{bot,scraper}:<sha>`. Note the SHA.

## 4. Roll it out (`luma-homeops`)
Bump `image.tag` to the step-3 SHA in
`cluster/applications/royaltracker-helm.yaml` and commit it inside
`nix-shell shell.nix --run 'git commit …'` (never `--no-verify`). Argo rolls the
bot pod (Recreate). Migrations 0012 and 0013 run on whichever of the bot or
scraper boots first on the new image.

### Sanity checks
- The bot starts cleanly. A failed migration makes it exit with a `db migrate` error.
- `/accounts` lists each existing user's account as `Royal Caribbean & Celebrity`.
- `/register` offers both `Royal Caribbean & Celebrity` and `Princess`.
- If the public tier is exposed, `GET /public/sailings?brand=princess&ship=<code>`
  returns voyages with codes.
- The next scraper run logs `scrape cycle complete` and `public sweep complete`.
