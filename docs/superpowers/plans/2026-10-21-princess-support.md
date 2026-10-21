# Princess Cruises Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Users own one cruise account per line (RCG or Princess). Princess bookings are tracked with shared watches and alerts, and Princess voyages can be looked up and subscribed to in the no-login public tier.

**Architecture:** Three shippable parts, each ending in a deploy checkpoint:

- **Part A** replaces the single-credential `users` row with a `cruise_accounts` table. It adds `Brand::Princess` to types and schema, brand-qualifies ship lookups, and folds in four fixes. There is no Princess network code yet.
- **Part B** adds a Princess API module. It has a guest SFCC price client, an account client for booking discovery, and dispatch by brand in discovery, scrape and catalog refresh.
- **Part C** adds the Princess schedule and voyage-code-keyed sailings to the public tier.

**Tech Stack:**

- Rust 1.88 workspace.
- axum 0.7.
- sqlx 0.8.6, with mutually exclusive `sqlite` / `postgres` features on `royaltracker-storage`.
- wreq 6.0.0-rc.28 with `wreq_util::Emulation::Chrome145`.
- teloxide 0.17.
- Preact/htm mini-app (`static/app.js`).
- Vanilla-JS public page (`static/public/index.html`).

**Spec:** `docs/superpowers/specs/2026-10-21-princess-support-design.md`. Read §5 (decided behavior), §6 (constraints) and §9 (Princess API reference) before any Part B/C task.

## Global Constraints

- Run every cargo command inside the flake devShell: `nix develop -c cargo …`.
  - Outside it, `boring-sys2` / `libsqlite3-sys` fail to build.
  - The bins default to `postgres`. Pass `--no-default-features --features sqlite` for SQLite.
- **Both backends.**
  - Every storage change lands in both `crates/royaltracker-storage/src/sqlite.rs` and `postgres.rs`.
  - Every schema change lands in both `migrations/sqlite/` and `migrations/postgres/`, with the same file number.
  - New migrations start at `0012`.
- **Postgres enum.** `ALTER TYPE brand_kind ADD VALUE` must be alone in its own migration file. It can't be used in the transaction that adds it.
- **Rollback safety.** Do NOT drop `users.rcg_username`, `rcg_password_ct`, `rcg_password_nonce` or `brand_pref` in this work. Make them nullable and stop reading or writing them.
- **User-Agent.** The `User-Agent` header must be `royaltracker_api::DEFAULT_USER_AGENT`, which matches the Chrome145 TLS emulation. Akamai flags mismatches.
- **No secrets in logs.** Never log `Authorization` headers, passwords, `appToken`s or SLAS tokens.
- **Public repo.** Committed fixtures must contain no names, DOBs, addresses, emails, loyalty numbers or real booking numbers. Use booking `AB12CD`, agency `00000000`, pax names `TEST`.
- **Princess logins:** one attempt per account per scrape run, with no automatic retry or backoff. Record the outcome on the account. RCG login behavior is unchanged.
- **Public role.** `cruise_accounts` is never granted to `royaltracker_public`. `ALTER DEFAULT PRIVILEGES … REVOKE` in `0011` already covers new tables; don't add grants.
- **Commits** end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Copy:**
  - Line labels are exactly `Royal Caribbean & Celebrity` (`rcg`) and `Princess` (`princess`).
  - Brand labels are `Royal Caribbean`, `Celebrity` and `Princess`.
  - The no-booking message is exactly: `Signed in to Princess, but couldn't open a booking — multi-booking accounts aren't supported yet.`

**Commands used throughout:**

- `TEST_STORAGE` = `nix develop -c cargo test -p royaltracker-storage --features sqlite`
- `CHECK_PG` = `nix develop -c cargo check -p royaltracker-storage -p royaltracker-web --features postgres && nix develop -c cargo check -p royaltracker-bot -p royaltracker-scraper`
- `TEST_PURE` = `nix develop -c cargo test -p royaltracker-types -p royaltracker-api -p royaltracker-core -p royaltracker-notify -p royaltracker-telegram`
- `BUILD_SQLITE` = `nix develop -c cargo build -p royaltracker-bot -p royaltracker-scraper --no-default-features --features sqlite`

## Review Focus

1. **A user removes their only cruise account.** Expected: the mini-app shows the link form, not an empty list or a 500, and they're unsubscribed from that line's bookings. *Test: Task 3 `remove_account_unsubscribes_line_bookings`. Task 6 frontend: zero accounts → register view.*
2. **A Princess login succeeds but the session has no booking context** (multi-booking or no upcoming trip). Expected: the exact no-booking message, an account error recorded, zero bookings, and no panic. *Tests: Task 9 `keepalive_without_booking_info_is_none`. Task 10 `princess_discovery_no_booking_records_error`.*
3. **A Princess voyage with zero priced products** (past, closed, or not yet open). Expected: treated as "nothing to record" (public: `PlannerNotOpen`). It must not become a $0 price or a failure counter that freezes the series. *Tests: Task 8 `empty_search_is_empty_vec`. Task 14 `princess_empty_catalog_is_planner_not_open`.*
4. **AUD voyage prices.** Expected: alerts, labels and the public table show `AUD 24.94`, never `$24.94`. *Tests: Task 2 `format_money_*`. Task 11 telegram render test.*
5. **Two Telegram users enter the same RCG login** (partners sharing one account). Expected: each gets their own `cruise_accounts` row, and both are subscribed to the shared bookings. *Test: Task 3 `same_login_two_users`.*

---

## Part A: Accounts model, Princess brand, fixes (ships alone; no Princess network code)

### Task 1: Migrations 0012–0013 (both backends) and the migration test

**Files:**
- Create: `migrations/postgres/0012_brand_princess.sql`, `migrations/postgres/0013_cruise_accounts.sql`
- Create: `migrations/sqlite/0012_brand_princess.sql`, `migrations/sqlite/0013_cruise_accounts.sql`
- Create: `crates/royaltracker-storage/tests/migrate_accounts.rs` (`#![cfg(feature = "sqlite")]`)
- Create: `crates/royaltracker-storage/tests/pg_migrate.rs` (`#![cfg(feature = "postgres")]`; skips unless `PG_TEST_URL` is set)

**Interfaces:**
- Produces this final schema, identical in logic on both backends:
  - `cruise_accounts`:
    - columns: `id` PK, `user_id` FK `users(id) ON DELETE CASCADE`, `cruise_line TEXT NOT NULL CHECK (cruise_line IN ('rcg','princess'))`, `login TEXT NOT NULL`, `password_ct`, `password_nonce`, `active` bool default true, `last_login_at` nullable timestamp, `last_login_error TEXT`, `created_at`, `updated_at`;
    - `UNIQUE (user_id, cruise_line)`.
  - `users`: `rcg_username`, `rcg_password_ct`, `rcg_password_nonce` and `brand_pref` become nullable. SQLite drops the `brand_pref` CHECK.
  - Brand: PG `brand_kind` gains `'princess'`; the SQLite brand CHECKs on `bookings` / `sailings` allow `'princess'`.
  - `bookings` gains `voyage_code TEXT` and `currency TEXT`.
  - `sailings` gains `voyage_code TEXT NOT NULL DEFAULT ''` and `currency TEXT`. Its unique key becomes `(brand, ship_code, sail_date, voyage_code)`.
  - `deck_plans` gains `brand`. PK becomes `(brand, ship_code, deck)`. The cache is emptied, since it repopulates lazily.
  - Backfill:
    - one `rcg` cruise account per existing `users` row with a non-null `rcg_username`, copying login, ciphertext, nonce and `active`;
    - every booking with `sailing_id IS NULL` gets linked to an upserted sailing (`voyage_code = ''`).

- [ ] **Step 1: Write the failing SQLite migration test** `crates/royaltracker-storage/tests/migrate_accounts.rs`.
  - Build a `sqlx::migrate::Migrator` from `sqlx::migrate!("../../migrations/sqlite")`. Keep only migrations with `version <= 11` by setting its `migrations` field to an owned filtered `Vec`.
  - Run it on a temp-file DB at `std::env::temp_dir()/rt-migrate-<random>.db`, with foreign keys on.
  - Insert legacy rows with raw SQL:
    - a user;
    - a `royal` booking with `sailing_id = NULL`;
    - a `booking_subscribers` row;
    - a `products_watched` row;
    - a `price_snapshots` row;
    - a `deck_plans` row.
  - Close the pool. Then call `royaltracker_storage::connect(url)` and `.migrate()`.

```rust
#[tokio::test] async fn legacy_user_becomes_rcg_account_and_data_survives() {
    // ... setup as above ...
    // Raw SQL only, so this task doesn't depend on Task 3's repo methods.
    let (line, login, ct): (String, String, Vec<u8>) = sqlx::query_as("SELECT cruise_line, login, password_ct FROM cruise_accounts WHERE user_id = ?").bind(user_id).fetch_one(&pool).await.unwrap();
    assert_eq!((line.as_str(), login.as_str(), ct), ("rcg", "legacy@example.com", b"ct".to_vec()));
    let count = |sql: &'static str| { let pool = pool.clone(); async move { sqlx::query_scalar::<_, i64>(sql).fetch_one(&pool).await.unwrap() } };
    assert_eq!(count("SELECT count(*) FROM booking_subscribers").await, 1);
    assert_eq!(count("SELECT count(*) FROM products_watched").await, 1);   // FK rebuild didn't cascade
    assert_eq!(count("SELECT count(*) FROM price_snapshots").await, 1);
    assert_eq!(count("SELECT count(*) FROM bookings WHERE sailing_id IS NOT NULL").await, 1);
    assert_eq!(count("SELECT count(*) FROM deck_plans").await, 0);        // cache emptied
    assert_eq!(count("PRAGMA foreign_key_check").await, 0);
}
#[tokio::test] async fn princess_combo_voyages_share_ship_and_date() {
    // Raw INSERTs: princess/ST/2027-01-02 with voyage_code '4702' and '4702A' both succeed;
    // a second '4702' row violates the unique key (assert the INSERT errors).
}
```

  - The test calls `connect()` + `migrate()` from the *current* storage crate, which still compiles at this point.
  - All assertions use a second `SqlitePool` on the same file.
- [ ] **Step 2: Run `TEST_STORAGE --test migrate_accounts`.** Expected: FAIL (no 0012/0013, or missing repo methods).
- [ ] **Step 3: Write `migrations/postgres/0012_brand_princess.sql`.** It contains only `ALTER TYPE brand_kind ADD VALUE IF NOT EXISTS 'princess';`.
- [ ] **Step 4: Write `migrations/sqlite/0012_brand_princess.sql`.**
  - First line: `-- no-transaction`. sqlx then runs it outside its wrapping transaction; that's needed because `PRAGMA foreign_keys` is a no-op inside a transaction.
  - Follow the SQLite 12-step rebuild for `users`, `bookings` and `sailings`:

```sql
-- no-transaction
PRAGMA foreign_keys=OFF;
BEGIN;
CREATE TABLE users_new (...same columns, rcg_* and brand_pref NULLABLE, brand_pref no CHECK...);
INSERT INTO users_new SELECT * FROM users;  DROP TABLE users;  ALTER TABLE users_new RENAME TO users;
-- bookings_new: brand CHECK IN ('royal','celebrity','princess'); all existing columns in their current order
--   (incl. user_id, nights, package_code, stateroom, assigned_stateroom, sailing_id), plus voyage_code TEXT, currency TEXT
-- sailings_new: brand CHECK incl 'princess'; voyage_code TEXT NOT NULL DEFAULT ''; currency TEXT;
--   UNIQUE (brand, ship_code, sail_date, voyage_code)
-- recreate every index that lived on these three tables (idx_users_active, idx_bookings_brand, idx_bookings_account,
--   idx_bookings_user, idx_sailings_active)
PRAGMA foreign_key_check;
COMMIT;
PRAGMA foreign_keys=ON;
```

  - Copy each table's column list from the migrations that built it (0001, 0002, 0004, 0008, 0010).
  - `INSERT … SELECT` must name columns explicitly. Don't use `SELECT *` for `bookings` and `sailings`, because the column order changes.
  - If sqlx 0.8.6 rejects `-- no-transaction` for SQLite, stop and report back. Don't fall back to a rebuild with foreign keys on: per the 0007 comment, that cascade-deletes snapshots.
- [ ] **Step 5: Write `migrations/{postgres,sqlite}/0013_cruise_accounts.sql`.**
  - Create `cruise_accounts` and backfill it from `users`.
  - PG:
    - `ALTER TABLE users ALTER COLUMN … DROP NOT NULL` on the four legacy columns;
    - `ALTER TABLE bookings ADD COLUMN voyage_code TEXT, ADD COLUMN currency TEXT`;
    - `ALTER TABLE sailings ADD COLUMN voyage_code TEXT NOT NULL DEFAULT '', ADD COLUMN currency TEXT`;
    - `ALTER TABLE sailings DROP CONSTRAINT sailings_brand_ship_code_sail_date_key, ADD CONSTRAINT sailings_identity_key UNIQUE (brand, ship_code, sail_date, voyage_code)`;
    - `DELETE FROM deck_plans; ALTER TABLE deck_plans ADD COLUMN brand brand_kind NOT NULL, DROP CONSTRAINT deck_plans_pkey, ADD PRIMARY KEY (brand, ship_code, deck)`.
  - SQLite: rebuild `deck_plans` in-transaction. Nothing references it, so `DROP TABLE` is safe with foreign keys on.
  - Both: link unlinked bookings:

```sql
INSERT INTO sailings (brand, ship_code, sail_date, voyage_code)
  SELECT DISTINCT brand, ship_code, sail_date, '' FROM bookings WHERE sailing_id IS NULL
  ON CONFLICT (brand, ship_code, sail_date, voyage_code) DO NOTHING;
UPDATE bookings SET sailing_id = (SELECT s.id FROM sailings s WHERE s.brand = bookings.brand AND s.ship_code = bookings.ship_code AND s.sail_date = bookings.sail_date AND s.voyage_code = '') WHERE sailing_id IS NULL;
```

- [ ] **Step 6: Write `tests/pg_migrate.rs`.**
  - Early-return unless `std::env::var("PG_TEST_URL")` is set.
  - Run all migrations, insert a legacy user via raw SQL before 0013 using the same filtered-migrator trick, and assert the cruise account exists.
  - Then insert two sailings `princess`/`ST`/`2027-01-02` with voyage `4702` and `4702A`.
- [ ] **Step 7: Run `TEST_STORAGE --test migrate_accounts`.** Expected: both tests PASS. Run only this test file: `public_flow.rs` fails until Task 3, because the old `upsert_sailing` `ON CONFLICT` target no longer exists.
- [ ] **Step 8: Verify Postgres migrations against a throwaway server.**

```bash
nix-shell -p postgresql_16 --run 'D=$(mktemp -d); initdb -D $D -U postgres >/dev/null; pg_ctl -D $D -o "-k $D -p 55432" -l $D/log start; createdb -h $D -p 55432 -U postgres rt'
PG_TEST_URL="postgres://postgres@localhost:55432/rt?host=$D" nix develop -c cargo test -p royaltracker-storage --features postgres --test pg_migrate
```

  - Expected: PASS. Stop the server afterwards with `pg_ctl -D $D stop`.
  - If the constraint name differs, `ALTER … DROP CONSTRAINT` errors loudly. Fix the name from `\d sailings`.
- [ ] **Step 9: Commit** `migrations: princess brand, cruise_accounts, sailing voyage_code, brand-qualified deck plans`.

### Task 2: Types (CruiseLine, Brand::Princess, CruiseAccount, brand-qualified ships, money format)

**Files:**
- Modify: `crates/royaltracker-types/src/lib.rs`
- Test: same file, `#[cfg(test)] mod tests`

**Interfaces:**
- Produces:
  - `pub enum CruiseLine { Rcg, Princess }`
    - serde `rename_all = "lowercase"`, so `"rcg"` / `"princess"`;
    - `as_str()`, `label()` (exact copy per Global Constraints), `brands(self) -> &'static [Brand]` (`Rcg` → `[Royal, Celebrity]`, `Princess` → `[Princess]`);
    - `Display`; `FromStr` accepting only `rcg|princess` (case-insensitive) with error `ParseCruiseLineError`.
  - `Brand::Princess`
    - `host()` = `"www.princess.com"`, `code()` = `"P"`, `url_segment()` = `"princess"`, `as_str()` = `"princess"`;
    - new `label()` (exact copy) and `line() -> CruiseLine`;
    - `FromStr` additionally accepts `princess|p|pcl`.
  - `pub struct CruiseAccount { id: i64, user_id: i64, line: CruiseLine, login: String, password_ct: Vec<u8>, password_nonce: Vec<u8>, active: bool, last_login_at: Option<DateTime<Utc>>, last_login_error: Option<String> }`.
  - `User` becomes `{ id, telegram_chat_id, telegram_username, active }`. The `rcg_*` and `brand_pref` fields are removed.
  - `Booking` gains `voyage_code: Option<String>` and `currency: Option<String>`, both `#[serde(default)]`.
  - `Sailing` gains `voyage_code: Option<String>` and `currency: Option<String>`.
  - Ship functions become brand-qualified:
    - `pub fn ship_name(brand: Brand, code: &str) -> Option<&'static str>`;
    - `all_ships()` gains the 17 Princess ships from spec §9.3, named e.g. `"Discovery Princess"`;
    - `pub fn cabin_deck_range(brand: Brand, ship_code: &str)`;
    - `pub fn cabin_location(brand: Brand, ship_code: &str, cabin: &str)`. Returns `None` for `Brand::Princess`.
  - `pub fn format_money(amount: f64, currency: Option<&str>) -> String`. `None` or `"USD"` gives `"$24.99"`; anything else gives `"{CUR} {amount:.2}"`, e.g. `"AUD 24.94"`.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn cruise_line_roundtrip() {
    for l in [CruiseLine::Rcg, CruiseLine::Princess] { assert_eq!(l.as_str().parse::<CruiseLine>().unwrap(), l); }
    assert!("royal".parse::<CruiseLine>().is_err());
    assert_eq!(CruiseLine::Rcg.brands(), &[Brand::Royal, Brand::Celebrity]);
    assert_eq!(Brand::Princess.line(), CruiseLine::Princess);
    assert_eq!(Brand::Celebrity.line(), CruiseLine::Rcg);
}
#[test] fn brand_princess_parses() { assert_eq!("princess".parse::<Brand>().unwrap(), Brand::Princess); assert_eq!(Brand::Princess.to_string(), "princess"); }
#[test] fn ship_codes_are_brand_qualified() {
    assert_eq!(ship_name(Brand::Royal, "EP"), Some("Explorer of the Seas"));
    assert_eq!(ship_name(Brand::Princess, "EP"), Some("Emerald Princess"));
    assert_eq!(ship_name(Brand::Celebrity, "SU"), Some("Celebrity Summit"));
    assert_eq!(ship_name(Brand::Princess, "SU"), Some("Sun Princess"));
    assert_eq!(ship_name(Brand::Princess, "OA"), None);
    assert_eq!(all_ships().iter().filter(|s| s.2 == Brand::Princess).count(), 17);
}
#[test] fn princess_has_no_cabin_location() { assert!(cabin_location(Brand::Princess, "XP", "9434").is_none()); assert!(cabin_location(Brand::Royal, "WN", "9434").is_some()); }
#[test] fn format_money_usd_and_aud() {
    assert_eq!(format_money(24.99, None), "$24.99");
    assert_eq!(format_money(24.99, Some("USD")), "$24.99");
    assert_eq!(format_money(24.94, Some("AUD")), "AUD 24.94");
}
```

Also update the existing `ship_name_known_and_unknown` test to the new signature.

- [ ] **Step 2: Run `nix develop -c cargo test -p royaltracker-types`.** Expected: compile FAIL (new items missing).
- [ ] **Step 3: Implement the interfaces above.**
  - Princess ship codes: AP Grand, CB Caribbean, CO Coral, DI Diamond, EP Emerald, EX Enchanted, GP Regal, IP Island, KP Crown, MJ Majestic, RP Royal, RU Ruby, SA Sapphire, ST Star, SU Sun, XP Discovery, YP Sky. Each name gets the suffix `Princess`.
  - `ship_name` is a match on `(brand, code)`.
- [ ] **Step 4: Run `nix develop -c cargo test -p royaltracker-types`.** Expected: PASS. Storage, core, web and the bins won't compile until Tasks 3–6 update their callers. That's expected: each later task compiles only the crates it has fixed.
- [ ] **Step 5: Commit** `types: CruiseLine, Brand::Princess, brand-qualified ships, format_money`.

### Task 3: Storage layer (accounts, guarded booking upsert, voyage-keyed sailings, scoped search)

**Files:**
- Modify: `crates/royaltracker-storage/src/repo.rs`, `src/sqlite.rs`, `src/postgres.rs`, `src/lib.rs` (re-exports)
- Modify: `crates/royaltracker-storage/tests/public_flow.rs` (new sailing signature)
- Create: `crates/royaltracker-storage/tests/accounts.rs` (`#![cfg(feature = "sqlite")]`)

**Interfaces:**
- Consumes: Task 2 types, Task 1 schema.
- Produces in `repo.rs`:
  - Users and accounts:
    - `NewUser<'a> { telegram_chat_id: i64, telegram_username: Option<&'a str> }`. `upsert_user` sets `active = true` on conflict.
    - `NewCruiseAccount<'a> { user_id: i64, line: CruiseLine, login: &'a str, password_ct: &'a [u8], password_nonce: &'a [u8] }`.
    - `pub struct ActiveAccount { pub user: User, pub account: CruiseAccount }`.
    - `async fn upsert_cruise_account(&self, a: &NewCruiseAccount<'_>) -> Result<i64, StorageError>`. On conflict `(user_id, cruise_line)` it replaces login and credentials, sets `active = true` and clears `last_login_error`.
    - `async fn list_cruise_accounts(&self, user_id: i64) -> Result<Vec<CruiseAccount>, StorageError>`.
    - `async fn list_active_cruise_accounts(&self) -> Result<Vec<ActiveAccount>, StorageError>`. Includes only active accounts whose user is active.
    - `async fn remove_cruise_account(&self, user_id: i64, line: CruiseLine) -> Result<bool, StorageError>`. Deletes the row and removes `booking_subscribers` rows for that user whose booking's brand is in `line.brands()`.
    - `async fn record_login_outcome(&self, account_id: i64, error: Option<&str>) -> Result<(), StorageError>`. Sets `last_login_at = now`, `last_login_error = error`.
    - Remove `set_user_brand` and `list_active_users`.
  - Bookings:
    - `StorageError::CrossLineConflict { reservation_id: String }`.
    - `upsert_booking` returns that error when an existing row's `brand.line()` differs from the new booking's. It also writes `voyage_code` / `currency`, and sets `sailing_id` by upserting the sailing `(brand, ship_code, sail_date, voyage_code or '')` in the same call.
  - Sailings:
    - `pub struct SailingKey<'a> { pub brand: Brand, pub ship_code: &'a str, pub sail_date: NaiveDate, pub voyage_code: Option<&'a str> }`. `None` is stored as `''`.
    - Signatures change to `upsert_sailing(&self, k: &SailingKey<'_>, currency: Option<&str>) -> Result<i64>` (currency via `COALESCE(excluded, existing)`), `get_sailing(&self, k: &SailingKey<'_>)` and `latest_sailing_prices(&self, k: &SailingKey<'_>)`.
    - `ScrapeTarget` and `PublicSubscriptionRow` gain `voyage_code: Option<String>` (`''` maps to `None`) and `currency: Option<String>`.
    - `PublicPriceDto` gains `currency: Option<String>` (from the sailing).
  - Catalog and deck plans:
    - `search_catalog(&self, user_id: i64, q: &str, limit: i64)` only returns entries whose `reservation_id` the user is subscribed to.
    - `DeckPlan` gains `brand: Brand`. `get_deck_plan(&self, brand: Brand, ship_code: &str, deck: i32)`.

- [ ] **Step 1: Write failing tests** in `tests/accounts.rs` (in-memory sqlite via `connect("sqlite::memory:")` + `migrate()`):

```rust
#[tokio::test] async fn upsert_account_replaces_creds_and_clears_error() { /* upsert, record_login_outcome(Some("bad")), upsert again with new ct → 1 row, new ct, last_login_error None */ }
#[tokio::test] async fn same_login_two_users() { /* two users, same rcg login → 2 accounts; booking subscribed to both via subscribe_user_to_booking → list_subscribers_for_reservation len 2 */ }
#[tokio::test] async fn remove_account_unsubscribes_line_bookings() { /* user with rcg+princess accounts, one royal + one princess booking subscribed; remove Rcg → list_bookings_for_user returns only the princess one; returns true; second remove returns false */ }
#[tokio::test] async fn inactive_user_accounts_not_listed() { /* deactivate_user → list_active_cruise_accounts empty */ }
#[tokio::test] async fn cross_line_booking_never_overwrites() { /* upsert royal booking "1234567"; upsert princess booking same id → Err(CrossLineConflict); royal row unchanged */ }
#[tokio::test] async fn upsert_booking_links_sailing() { /* princess booking voyage Some("X637") currency Some("USD") → get_sailing(SailingKey{voyage_code:Some("X637")..}) is Some with currency "USD" */ }
#[tokio::test] async fn search_catalog_scoped_to_user() { /* two users, two reservations, catalog rows "Soda" on each → user A's search returns only A's reservation */ }
```

- [ ] **Step 2: Run `TEST_STORAGE`.** Expected: compile FAIL.
- [ ] **Step 3: Implement** the interfaces in `repo.rs`, `sqlite.rs` and `postgres.rs`.
  - Update `public_flow.rs` to `SailingKey`.
  - PG brand binds keep the `$n::brand_kind` casts.
- [ ] **Step 4: Run `TEST_STORAGE`.** Expected: PASS, including `public_flow.rs` and Task 1's `migrate_accounts.rs`.
- [ ] **Step 5: Run `nix develop -c cargo check -p royaltracker-storage --features postgres`.** Expected: PASS.
- [ ] **Step 6: Commit** `storage: cruise accounts, cross-line booking guard, voyage-keyed sailings, scoped catalog search`.

### Task 4: Core discovery per account and scraper loop

**Files:**
- Modify: `crates/royaltracker-core/src/bookings.rs`, `src/scrape.rs` (`itinerary_label` → `ship_name(b.brand, …)`), `src/public_sweep.rs` (`sailing_label` gains `brand`), `src/lib.rs`
- Modify: `bins/royaltracker-scraper/src/main.rs`

**Interfaces:**
- Consumes: Task 3 `ActiveAccount`, `record_login_outcome`, `upsert_booking`.
- Produces:
  - `pub struct AccountDiscovery { pub report: DiscoveryReport, pub rcg_clients: HashMap<Brand, CruiseClient> }`.
  - `pub async fn discover_account(repo: &dyn PriceRepo, rcg_basic_auth_b64: &str, user: &User, account: &CruiseAccount, password: &str, notifier: Option<&dyn Notifier>) -> anyhow::Result<AccountDiscovery>`.
    - Dispatches on `account.line`.
    - `Rcg` calls the existing `discover_with_clients(account.login, password, …)`. Its signature is unchanged except that the final `info!` logs `account.id`, not a username.
    - `Princess` returns `Err(anyhow!("Princess support is not enabled yet"))` until Task 10.
    - Always calls `repo.record_login_outcome(account.id, …)`. The outcome is `None` when `report.logins_ok > 0`; otherwise the joined errors (or the `Err` text).
  - `discover_and_persist_bookings` is removed. Web callers use `discover_account` (Task 5).
  - `discover_with_clients` skips a booking whose `upsert_booking` returns `CrossLineConflict`, with `warn!`, and pushes it to `report.errors`. It still persists every other booking.
- Scraper:
  - `main.rs` replaces `list_active_users` with `list_active_cruise_accounts()`.
  - Per `ActiveAccount`: decrypt `account.password_ct` with the nonce, call `discover_account`, and merge `rcg_clients` first-wins into `clients_by_brand`.
  - On `Err`, send the existing Telegram warning to `active.user.telegram_chat_id`.
  - The early return becomes "no active accounts".

- [ ] **Step 1: Write failing unit tests** in `bookings.rs` for the outcome rule. Factor it into `pub(crate) fn login_outcome(report: &DiscoveryReport) -> Option<String>`; the `PriceRepo` trait is too large to stub for a unit test.

```rust
#[test] fn login_outcome_ok_when_any_login_succeeded() { let r = DiscoveryReport { persisted: 1, logins_ok: 1, errors: vec!["celebrity: x".into()] }; assert_eq!(login_outcome(&r), None); }
#[test] fn login_outcome_err_when_no_login() { let r = DiscoveryReport { persisted: 0, logins_ok: 0, errors: vec!["royal: 401".into(), "celebrity: 401".into()] }; assert_eq!(login_outcome(&r).as_deref(), Some("royal: 401; celebrity: 401")); }
```

- [ ] **Step 2: Run `nix develop -c cargo test -p royaltracker-core`.** Expected: FAIL.
- [ ] **Step 3: Implement `login_outcome`, `discover_account`, the conflict skip, and the call-site updates in core and the scraper.**
- [ ] **Step 4: Run `nix develop -c cargo test -p royaltracker-core`.** Expected: PASS. Run `nix develop -c cargo check -p royaltracker-scraper --no-default-features --features sqlite`. Expected: PASS.
- [ ] **Step 5: Commit** `core: discovery per cruise account; scraper iterates accounts`.

### Task 5: Web API (accounts endpoints, register/refresh/me, the four fixes)

**Files:**
- Modify: `crates/royaltracker-web/src/handlers.rs`, `src/lib.rs` (routes)

**Interfaces:**
- Consumes: Tasks 3–4.
- Produces (Rust helpers in `handlers.rs`):
  - `fn validate_register(line: CruiseLine, login: &str, password: &str) -> Result<(), ApiError>`.
  - `impl From<&CruiseAccount> for AccountDto`.
- Produces (HTTP):
  - **Register.** `POST /api/register` body `{ cruise_line?: "rcg"|"princess" (default "rcg"), login: string (serde alias "email"), password: string }`.
    - Validation: empty password → 400. For `rcg` the login must contain `@` → 400 `"invalid email"`.
    - Flow: `upsert_user`, then `upsert_cruise_account`, then `discover_account`.
    - The response shape is unchanged (`login_ok`, `bookings_discovered`, `message`).
  - **Refresh.** `POST /api/refresh` runs `discover_account` for every active account of the caller. Response `{ bookings_discovered: usize (sum), errors: Vec<String> }`, each error prefixed with the line label.
  - **Accounts.**
    - `GET /api/accounts` → `Vec<AccountDto { cruise_line: String, label: String, login: String, active: bool, last_login_at: Option<DateTime<Utc>>, last_login_error: Option<String> }>`.
    - `DELETE /api/accounts/:line` → 204, or 404 if the line had no account.
  - **Me.** `GET /api/me` → `{ tg_id, tg_username, tg_first_name, accounts: Vec<AccountDto> }`. `rcg_username` and `brand` are removed.
  - **Bookings.** `BookingDto` gains `ship_name: Option<String>` (from `ship_name(b.brand, …)`), `brand_label: String` and `voyage_code: Option<String>`. Location uses `cabin_location(b.brand, …)`.
  - **Fix 1: catalog refresh.** `refresh_catalog` builds the `CruiseClient` with `booking.brand`. For `Brand::Princess` it returns 501 until Task 11.
  - **Fix 3: catalog scoping.**
    - `search_catalog` passes `user.db_user.id`.
    - `browse_catalog` returns 404 unless `user_owns_reservation`.
  - **Fix 4: deck plans.**
    - `DeckPlanQuery` gains `brand: String`. Ownership requires a booking matching both brand and ship.
    - `Brand::Princess` → 404 `"deck plans not available"`.
    - Cache reads and writes include `brand`.

- [ ] **Step 1: Write failing tests** as handler-level unit tests on pure helpers. Extract and test:

```rust
#[test] fn register_validation() {
    assert!(validate_register(CruiseLine::Rcg, "a@b.c", "pw").is_ok());
    assert!(validate_register(CruiseLine::Rcg, "nope", "pw").is_err());
    assert!(validate_register(CruiseLine::Princess, "LOGINID", "pw").is_ok());   // Princess login id needn't be an email
    assert!(validate_register(CruiseLine::Princess, "x", "").is_err());
}
#[test] fn account_dto_never_contains_secrets() { let json = serde_json::to_string(&AccountDto::from(&sample_account())).unwrap(); assert!(!json.contains("password")); assert!(!json.contains("ct")); }
```

- [ ] **Step 2: Run `nix develop -c cargo test -p royaltracker-web --features sqlite`.** Expected: FAIL.
- [ ] **Step 3: Implement the endpoints and fixes.** Decrypt with `s.cipher.decrypt(&acct.password_nonce, &acct.password_ct)`.
- [ ] **Step 4: Run `nix develop -c cargo test -p royaltracker-web --features sqlite`.** Expected: PASS.
- [ ] **Step 5: Commit** `web: accounts API; fix celebrity catalog brand, catalog scoping, brand-qualified deck plans`.

### Task 6: Mini-app and bot UX for accounts (RCG only exposed)

**Files:**
- Modify: `crates/royaltracker-web/static/app.js`
- Modify: `bins/royaltracker-bot/src/main.rs`

**Interfaces:**
- Consumes: Task 5 HTTP, Task 3 repo.
- Mini-app:
  - `RegisterForm` becomes `AccountForm({ onLinked, lines })`. It has a `<select>` of lines. Part A passes `lines = [["rcg", "Royal Caribbean & Celebrity"]]`; Task 12 adds Princess.
  - Labels: `Login` (email for RCG), `Password`.
  - It posts `{ cruise_line, login, password }`.
  - The bootstrap shows the form when `/api/me` is 403 OR `me.accounts.length === 0`.
  - A new `AccountsView` is reachable from the header ("Accounts"). It lists `label · login · ✅ / ⚠ last_login_error`, with Remove (DELETE) and "Add account" (renders `AccountForm`).
  - `BRAND_LABEL` gains `princess: "Princess"`.
  - `shipName(b)` uses `b.ship_name || b.ship_code`. Delete the `SHIP_NAMES` table.
  - The deck-plan slug derives from `booking.ship_name`, and `/api/deckplan` gets `&brand=${booking.brand}`.
  - Hide the location/deck panel when `booking.brand === "princess"`.
  - `Header`'s subtitle becomes `${first name or username} · ${me.accounts.map(a => a.label).join(" + ")}`. When signed out it reads `Welcome — link a cruise account to begin.`; `rcg_username` is gone.
- Bot:
  - `fn line_keyboard(lines: &[CruiseLine]) -> InlineKeyboardMarkup`: one button per line, text `line.label()`, callback `line:{line}`.
  - `DialogueState` becomes `Idle | AwaitingLine | AwaitingLogin { line: CruiseLine } | AwaitingPassword { line: CruiseLine, login: String }`.
    - `/register` sends an inline keyboard of enabled lines (Part A: `line:rcg` only).
    - The `line:*` callback moves to `AwaitingLogin`.
  - On password:
    - `upsert_user`, then `upsert_cruise_account`;
    - reply `✅ Stored. Testing {label} login…`;
    - test the login for that line (RCG: existing `CruiseClient` login on `Brand::Royal`);
    - `record_login_outcome`.
  - New `/accounts` command lists accounts, with inline `Remove {label}` buttons (callback `rmacct:{line}`) calling `remove_cruise_account`.
  - `/test` tests every account.
  - Command descriptions:
    - `Register`: `link a cruise line account (Royal Caribbean & Celebrity, or Princess)`;
    - `Accounts`: `list or remove your linked cruise accounts`.

- [ ] **Step 1: Write a failing test** in `main.rs` `#[cfg(test)]` for the keyboard builder:

```rust
#[test] fn line_keyboard_uses_enabled_lines() { let kb = line_keyboard(&[CruiseLine::Rcg]); let json = serde_json::to_string(&kb).unwrap(); assert!(json.contains("line:rcg")); assert!(!json.contains("line:princess")); assert!(json.contains("Royal Caribbean & Celebrity")); }
```

- [ ] **Step 2: Run `nix develop -c cargo test -p royaltracker-bot --no-default-features --features sqlite`.** Expected: FAIL.
- [ ] **Step 3: Implement the bot changes and app.js changes.**
- [ ] **Step 4: Run the bot tests (PASS), then `BUILD_SQLITE` and `CHECK_PG` (PASS).**
- [ ] **Step 5: Manual smoke.**
  - Run `DEV.md`'s `nix develop -c cargo run -q -p royaltracker-bot --no-default-features --features sqlite` against a copy of `dev.db`: `cp dev.db /tmp/rt-dev-copy.db` and point config at it.
  - Confirm the existing user's bookings still list in the mini-app.
  - Confirm `/accounts` shows one `Royal Caribbean & Celebrity` account.
- [ ] **Step 6: Commit** `ux: link/remove cruise accounts in mini-app and bot`.

### Task 7: Part A gate

- [ ] **Step 1:** Run `TEST_STORAGE && TEST_PURE && BUILD_SQLITE && CHECK_PG` and `nix develop -c cargo test -p royaltracker-web --features sqlite`. Expected: all PASS.
- [ ] **Step 2:** Run `nix develop -c cargo clippy -p royaltracker-storage --features sqlite -p royaltracker-core -p royaltracker-types -- -D warnings`. Expected: clean.
- [ ] **Step 3: Deploy checkpoint. Ask the user before pushing or deploying.**
  - The deploy is the two-repo flow: royaltracker `main` → image; bump `image.tag` in `luma-homeops/cluster/applications/royaltracker-helm.yaml`; luma-homeops commits go via `nix-shell shell.nix --run 'git commit …'`; Argo syncs.
  - After rollout, verify in the prod DB that `SELECT cruise_line, count(*) FROM cruise_accounts GROUP BY 1` equals the active user count.

---

## Part B: Princess client and logged-in tracking

### Task 8: Princess guest price client (SFCC)

**Files:**
- Create: `crates/royaltracker-api/src/princess/mod.rs`, `src/princess/catalog.rs`
- Modify: `crates/royaltracker-api/src/lib.rs` (`pub mod princess;` re-exports), `src/public.rs` (`PublicProduct` gains `child_cents: Option<i64>`, set `None` in the RCG path)
- Create fixtures: `crates/royaltracker-api/tests/fixtures/princess/search_excursions.json`, `search_wifi.json`, `products_wifi.json`, `search_food_aud.json`, `search_empty.json`
- Test: `crates/royaltracker-api/tests/princess_catalog.rs`

**Interfaces:**
- Produces:
  - Constants in `princess/mod.rs`:
    - `PCL_CLIENT_ID = "32e7224ac6cc41302f673c5f5d27b4ba"`
    - `SLAS_CLIENT_ID = "25ddb611-56bf-4c76-85e1-c7c83c412085"`
    - `SLAS_PLACEHOLDER_SECRET = "_PLACEHOLDER_PROXY-PWA_KIT_SLAS_CLIENT_SECRET"`
    - `SFCC_ORG = "f_ecom_blnk_prd"`
    - `SFCC_SITE = "PCL"`
    - `EXTRAS_CATEGORIES = ["food-and-drinks", "wifi", "extras"]`
  - `pub struct PrincessGuestClient`:
    - `pub fn new(pacing: PacingConfig) -> Result<Self, ApiError>` (uses `build_emulated_client()` + `DEFAULT_USER_AGENT`);
    - `pub async fn fetch_excursions(&self, voyage_code: &str, currency: &str) -> Result<Vec<PublicProduct>, ApiError>`;
    - `pub async fn fetch_extras(&self, currency: &str) -> Result<Vec<PublicProduct>, ApiError>`.
  - `pub fn parse_search_hits(v: &serde_json::Value, category_id: &str, currency: &str) -> Vec<PublicProduct>`.
  - `pub fn apply_product_details(products: &mut [PublicProduct], v: &serde_json::Value)`. Fills `unit_label` from the detail `unit`, as `"per person / day"`.
- Behavior (spec §9.2):
  - **Guest token.** Fetched lazily and reused until 60 s before `expires_in`. No shopper context is ever set.
  - **Search.** Pages with `limit=200`, advancing `offset` until `offset >= total`.
  - **Mapping:**
    - `product_code = productId`, `title = productName`, `category_id` = the cgid; `category_name`: `excursions` → `"Excursions"`, `food-and-drinks` → `"Food & Drinks"`, `wifi` → `"Wi-Fi"`, `extras` → `"Extras"`;
    - `promo_cents = round(price*100)`;
    - `base_cents` = the `maxPrice` of a `c_priceRanges` entry whose pricebook contains `-list`, only when greater than `price`;
    - `child_cents` from a pricebook containing `-child`;
    - labels via `royaltracker_types::format_money`; `currency` set;
    - excursions `unit_label = "per adult"`.
  - **Extras detail.** `fetch_extras` calls the products detail endpoint in batches of 24 ids and applies `apply_product_details`.
  - **Errors.** A body matching `is_bot_challenge` → `Err(ApiError::Status{…})`, same as the RCG public client.

- [ ] **Step 1: Capture fixtures.** Use the guest token flow in spec §9.2 with `curl`, against a public voyage that has excursions and sails at least a month out (e.g. `4702`, Star Princess 2027-01-02; check it is still in the schedule), plus one AUD voyage. Save raw JSON to the fixture paths. These responses contain no PII; check with `grep -iE '"(email|firstName|lastName|birth)' fixtures/*` (expected: none). Write `search_empty.json` as `{"limit":200,"hits":[],"offset":0,"total":0}`.
- [ ] **Step 2: Write failing tests**

```rust
#[test] fn excursion_hit_maps_price_and_unit() { let p = parse(&fixture("search_excursions.json"), "excursions", "USD"); assert!(!p.is_empty()); let x = &p[0]; assert!(x.promo_cents.unwrap() > 0); assert_eq!(x.unit_label.as_deref(), Some("per adult")); assert_eq!(x.currency.as_deref(), Some("USD")); assert!(x.promo_label.as_deref().unwrap().starts_with('$')); }
#[test] fn wifi_list_price_becomes_base() { let p = parse(&fixture("search_wifi.json"), "wifi", "USD"); let w = p.iter().find(|p| p.product_code == "wifi-single-device").unwrap(); assert!(w.base_cents.unwrap() > w.promo_cents.unwrap()); }
#[test] fn details_fill_per_day_unit() { let mut p = parse(&fixture("search_wifi.json"), "wifi", "USD"); apply_product_details(&mut p, &fixture("products_wifi.json")); assert_eq!(p[0].unit_label.as_deref(), Some("per person / day")); }
#[test] fn aud_labels_use_code() { let p = parse(&fixture("search_food_aud.json"), "food-and-drinks", "AUD"); assert!(p[0].promo_label.as_deref().unwrap().starts_with("AUD ")); }
#[test] fn empty_search_is_empty_vec() { assert!(parse(&fixture("search_empty.json"), "excursions", "USD").is_empty()); }
```

- [ ] **Step 3: Run `nix develop -c cargo test -p royaltracker-api --test princess_catalog`.** Expected: FAIL.
- [ ] **Step 4: Implement `princess/mod.rs` and `princess/catalog.rs`.**
- [ ] **Step 5: Run the test (PASS). Then run a live check with a new example `crates/royaltracker-api/examples/probe_princess.rs`.** It prints the counts from `fetch_excursions("4702","USD")` and `fetch_extras("USD")`. Run it with `nix develop -c cargo run -p royaltracker-api --example probe_princess`. Expected: excursions > 0, extras ≈ 44.
- [ ] **Step 6: Commit** `api: Princess guest SFCC price client`.

### Task 9: Princess account client (login + booking context)

**Files:**
- Create: `crates/royaltracker-api/src/princess/account.rs`
- Create fixtures: `tests/fixtures/princess/keepalive.json` (hand-written, scrubbed: booking `AB12CD`, agency `00000000`, ship `XP`, voyage `X637`, paxId `1`, durationDays `7`, departureDate `Saturday, October 24, 2026`, bookingCurrency `USD`, appToken `tok`) and `keepalive_no_booking.json` (`{"data":{"httpcode":201,"tokenObj":{"appToken":"tok","expiresIn":30},"tokenStatus":"Created"},"status":"success"}`)
- Test: `crates/royaltracker-api/tests/princess_account.rs`; live `crates/royaltracker-api/tests/princess_login_smoke.rs` (skips unless `PRINCESS_LOGIN` and `PRINCESS_PASSWORD` are set)

**Interfaces:**
- Produces:
  - `pub struct PrincessBookingContext { pub booking_id: String, pub ship_code: String, pub voyage_code: String, pub pax_id: String, pub nights: Option<i32>, pub sail_date: NaiveDate, pub currency: String }`.
  - `pub fn basic_auth_value(login: &str, password: &str) -> String`. Returns `"Basic " + base64(urlencode(login) + ":" + urlencode(password))`, using `url::form_urlencoded::byte_serialize` for each part.
  - `pub fn parse_departure(s: &str) -> Option<NaiveDate>`, format `"%A, %B %d, %Y"`.
  - `pub fn parse_keep_alive(v: &serde_json::Value) -> Option<PrincessBookingContext>`. Returns `None` when `data.bookingInfo` is absent or lacks `bookingId`.
  - `pub struct PrincessAccountClient`:
    - `pub fn new() -> Result<Self, ApiError>`;
    - `pub async fn login_and_context(&self, login: &str, password: &str) -> Result<Option<PrincessBookingContext>, ApiError>`. Performs exactly one login attempt (spec §9.1 steps 1–3), with no retry: warm-up `www.princess.com`, `POST …/guest/p1.0/login`, `GET …/captaincircle/quickLogin.do`, then `GET …/manage-booking/json/sfccKeepAlive.page`.
    - Login non-200 → `Err(ApiError::Status)`, with the body truncated to 200 chars.
    - Princess's `booking/display/menu` / tier call is NOT made; prices are tier-agnostic (spec §5.4), so nothing would consume it.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn basic_auth_urlencodes_both_parts() { assert_eq!(basic_auth_value("a@b.com", "p#1!"), format!("Basic {}", base64::encode("a%40b.com:p%231%21"))); }
#[test] fn departure_text_parses() { assert_eq!(parse_departure("Saturday, October 24, 2026"), NaiveDate::from_ymd_opt(2026,10,24)); }
#[test] fn keepalive_parses_context() { let c = parse_keep_alive(&fixture("keepalive.json")).unwrap(); assert_eq!(c.booking_id, "AB12CD"); assert_eq!(c.voyage_code, "X637"); assert_eq!(c.pax_id, "1"); assert_eq!(c.nights, Some(7)); assert_eq!(c.currency, "USD"); }
#[test] fn keepalive_without_booking_info_is_none() { assert!(parse_keep_alive(&fixture("keepalive_no_booking.json")).is_none()); }
```

  Use whatever base64 API the crate already uses; it depends on `base64` (see `ApiError::Base64`).
- [ ] **Step 2: Run `nix develop -c cargo test -p royaltracker-api --test princess_account`.** Expected: FAIL.
- [ ] **Step 3: Implement `account.rs`.**
- [ ] **Step 4: Run the test.** Expected: PASS.
- [ ] **Step 5: Live smoke test (needs the user's Princess login, so ask the user to run it).**
  - Command: `PRINCESS_LOGIN=… PRINCESS_PASSWORD=… nix develop -c cargo test -p royaltracker-api --test princess_login_smoke -- --nocapture`.
  - Expected: `Some(context)` with a 6-char booking id.
  - Run it once. Don't loop: there's an unknown rate limit (spec §8.4).
- [ ] **Step 6: Commit** `api: Princess account login and booking context`.

### Task 10: Princess discovery

**Files:**
- Create: `crates/royaltracker-core/src/princess.rs`
- Modify: `crates/royaltracker-core/src/bookings.rs` (`discover_account` Princess arm), `src/lib.rs`

**Interfaces:**
- Consumes: Task 9 `PrincessAccountClient`, `PrincessBookingContext`; Task 3 `upsert_sailing`, `upsert_booking`, `subscribe_user_to_booking`.
- Produces:
  - `pub const PRINCESS_NO_BOOKING_MSG: &str` (exact copy per Global Constraints).
  - `pub fn booking_from_context(ctx: &PrincessBookingContext, account_id: i64) -> Booking`. Fields:
    - `brand: Princess`;
    - `reservation_id = booking_id`;
    - `account_id = format!("princess:{account_id}")`;
    - `passenger_id = Some(pax_id)`;
    - `nights`;
    - `voyage_code = Some(voyage_code)`;
    - `currency = Some(currency)`;
    - `stateroom` / `assigned_stateroom` / `package_code` = `None`.
  - `pub async fn discover_princess(repo: &dyn PriceRepo, user: &User, account: &CruiseAccount, password: &str) -> anyhow::Result<DiscoveryReport>`.
    - Context `None` → returns a report with `logins_ok = 1`, `persisted = 0`, `errors = [PRINCESS_NO_BOOKING_MSG]`.
    - Login error → `Err`.
    - Otherwise: upsert the sailing (`SailingKey { brand: Princess, ship_code, sail_date, voyage_code: Some }`, currency), upsert the booking, subscribe the user. Report `persisted = 1`.
  - `discover_account`'s `Princess` arm calls it and records the outcome.
    - For the no-booking case, the recorded error is `PRINCESS_NO_BOOKING_MSG`, even though `logins_ok = 1`. `login_outcome` gains this special case: if errors contain `PRINCESS_NO_BOOKING_MSG`, return it.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn booking_from_context_maps_fields() { let b = booking_from_context(&ctx(), 7); assert_eq!(b.brand, Brand::Princess); assert_eq!(b.reservation_id, "AB12CD"); assert_eq!(b.account_id, "princess:7"); assert_eq!(b.voyage_code.as_deref(), Some("X637")); assert_eq!(b.currency.as_deref(), Some("USD")); }
#[test] fn princess_discovery_no_booking_records_error() { let r = DiscoveryReport { persisted: 0, logins_ok: 1, errors: vec![PRINCESS_NO_BOOKING_MSG.into()] }; assert_eq!(login_outcome(&r).as_deref(), Some(PRINCESS_NO_BOOKING_MSG)); }
```

- [ ] **Step 2: Run `nix develop -c cargo test -p royaltracker-core`.** Expected: FAIL.
- [ ] **Step 3: Implement it.**
- [ ] **Step 4: Run it.** Expected: PASS.
- [ ] **Step 5: Commit** `core: Princess booking discovery`.

### Task 11: Princess prices in the scrape cycle, catalog refresh, currency-aware alerts

**Files:**
- Modify: `crates/royaltracker-core/src/princess.rs` (price cache), `src/scrape.rs`, `src/catalog.rs`
- Modify: `crates/royaltracker-notify/src/lib.rs` (`PriceDropAlert` gains `currency: Option<&'a str>`), `src/webpush.rs`, `crates/royaltracker-telegram/src/lib.rs`
- Modify: `bins/royaltracker-scraper/src/main.rs`, `crates/royaltracker-web/src/handlers.rs` (`refresh_catalog` Princess arm), `crates/royaltracker-web/src/state.rs` (`AppState` gains `pub princess: Arc<PrincessGuestClient>`), `bins/royaltracker-bot/src/main.rs` (constructs it)

**Interfaces:**
- Consumes: Task 8 `PrincessGuestClient`, Task 10.
- Produces:
  - `pub struct PrincessPriceCache` (`Default`). `pub async fn products(&mut self, client: &PrincessGuestClient, voyage_code: &str, currency: &str) -> Result<HashMap<String, PublicProduct>, ApiError>`.
    - Returns excursions for `(voyage, currency)` merged with extras for `currency`, keyed by `product_code`.
    - Each excursions fetch and each extras fetch happens at most once per cache instance.
  - `run_scrape_cycle` gains `princess: Option<&PrincessGuestClient>` as its second parameter. Princess bookings never use `clients_by_brand`.
    - The `Brand::Princess` path uses a local `PrincessPriceCache`, `booking.voyage_code`, and `booking.currency` (default `"USD"`).
    - Product missing → `warn!` and continue.
    - The snapshot is `adult_promo_price = promo_dollars()`, `child_promo_price = child_cents/100`, `raw_response = to_value(product)`.
    - The diff and fan-out logic is unchanged.
    - The alert passes `currency: booking.currency.as_deref()`.
  - `pub async fn refresh_princess_catalog(client: &PrincessGuestClient, repo: &dyn PriceRepo, booking: &Booking) -> anyhow::Result<usize>`. Upserts `CatalogEntry`, with `price_label` / `base_price_label` / `unit_label` / `currency` from `PublicProduct`.
  - Formatting: telegram and webpush render prices with `format_money(price, alert.currency)` in place of the hardcoded `"$"`. RCG callers pass `None`.
    - Telegram's `DiffContext` gains `currency: Option<&'a str>`.
    - Move the message building out of `send_diff` into `fn render_diff(ctx: &DiffContext<'_>) -> String` (MarkdownV2-escaped, as today) so it can be tested.

- [ ] **Step 1: Write failing tests**

```rust
// royaltracker-telegram
#[test] fn aud_drop_renders_currency_code() { let d = Diff::from_prices(1, 30.0, 24.94); let text = render_diff(&DiffContext { currency: Some("AUD"), ..ctx("Classic Soda", &d) }); assert!(text.contains("AUD 24\\.94")); assert!(!text.contains("$24")); }
// royaltracker-core princess.rs
#[test] fn merge_prefers_excursion_and_extras_by_code() { let m = merge_products(vec![prod("SJD-100")], vec![prod("2323")]); assert!(m.contains_key("SJD-100") && m.contains_key("2323")); }
```

  `ctx(label, &diff)` is a `#[cfg(test)]` helper that builds a `DiffContext` with every other field `None`. `merge_products(excursions: Vec<PublicProduct>, extras: Vec<PublicProduct>) -> HashMap<String, PublicProduct>` is the pure core of `PrincessPriceCache::products`.
- [ ] **Step 2: Run `TEST_PURE`.** Expected: FAIL.
- [ ] **Step 3: Implement it.** The scraper constructs `PrincessGuestClient::new(pacing(&cfg.pacing))` once and passes it to both `run_scrape_cycle` and (in Part C) the public sweep.
- [ ] **Step 4: Run `TEST_PURE` (PASS), then `BUILD_SQLITE` and `CHECK_PG` (PASS).**
- [ ] **Step 5: Commit** `core: Princess price fetch in scrape + catalog refresh; currency-aware alerts`.

### Task 12: Expose Princess in the bot and mini-app; Part B gate

**Files:**
- Modify: `bins/royaltracker-bot/src/main.rs` (enabled lines include `Princess`; `/test` Princess arm uses `PrincessAccountClient::login_and_context`), `crates/royaltracker-web/static/app.js` (`lines` includes `["princess", "Princess"]`; the Login label reads `Login ID or email` when Princess is selected)

- [ ] **Step 1:** Update the `line_keyboard_uses_enabled_lines` test to expect both `line:rcg` and `line:princess`. Run the bot tests and confirm they FAIL.
- [ ] **Step 2:** Implement the changes. Run the bot tests (PASS), `BUILD_SQLITE` and `CHECK_PG` (PASS).
- [ ] **Step 3: End-to-end manual check (needs the user's Princess login, so ask the user).** On a dev copy DB:
  1. link Princess in the mini-app; expected: 1 booking appears labelled Princess with the correct ship name;
  2. refresh the catalog; expected: products listed with per-day units;
  3. watch a product;
  4. run the scraper once (`nix develop -c cargo run -p royaltracker-scraper --no-default-features --features sqlite`); expected: a snapshot is written for the Princess watch and the log shows one excursions fetch plus one extras fetch.
- [ ] **Step 4: Commit** `ux: enable Princess accounts`.
- [ ] **Step 5: Deploy checkpoint (ask the user).** Same flow as Task 7.

---

## Part C: Princess in the public tier

### Task 13: Princess schedule and the public sailings endpoint

**Files:**
- Create: `crates/royaltracker-api/src/princess/schedule.rs`; fixture `tests/fixtures/princess/schedule_sample.json`. Trim a live response to the itinerary products containing voyages `4702`, `4702A` (ST 2027-01-02) and one AUD voyage, keeping their `cruises` arrays.
- Test: `crates/royaltracker-api/tests/princess_schedule.rs`
- Modify: `crates/royaltracker-web/src/public.rs` (`sailings` handler, `PublicState` gains a schedule cache), `bins/royaltracker-bot/src/main.rs` and `crates/royaltracker-web/examples/serve_public.rs` (both construct `PublicState`)
- Modify: `crates/royaltracker-api/src/public.rs`. The RCG `fetch_public_sailings` stays as-is; the web layer adapts it.

**Interfaces:**
- Produces:
  - `pub struct PrincessVoyage { pub voyage_code: String, pub ship_code: String, pub sail_date: NaiveDate, pub nights: i32, pub itinerary: String, pub currency: String }`.
  - `pub fn parse_schedule(v: &serde_json::Value) -> Vec<PrincessVoyage>`. Rules:
    - `currency` comes from `shipCurrency`, defaulting to `"USD"` when empty;
    - the result is sorted by `(ship_code, sail_date, voyage_code)`;
    - `sailDate` is `%Y%m%d`.
  - `PrincessGuestClient::fetch_schedule(&self) -> Result<Vec<PrincessVoyage>, ApiError>`. Calls `GET https://gw.api.princess.com/pcl-web/internal/resdb/p1.0/products?agencyCountry=US&cruiseType=C&voyageStatus=A&webDisplay=Y&promoFilter=all&light=false`, with the common gateway headers (spec §9).
  - Web:
    - `PublicState` gains `princess: Arc<PrincessGuestClient>` and `schedule: Arc<tokio::sync::RwLock<Option<(Instant, Vec<PrincessVoyage>)>>>`, refreshed when older than 24 h.
    - `GET /public/sailings?brand=&ship=` returns `Vec<SailingOption { sail_date: String (YYYY-MM-DD), voyage_code: Option<String>, nights: Option<i32>, itinerary: Option<String> }>` for every brand.
    - RCG maps its dates with the three optional fields set to `None`.
    - Princess filters the cached schedule by ship and keeps only `sail_date >= today`.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn schedule_parses_combo_voyages() { let v = parse_schedule(&fixture("schedule_sample.json")); let st: Vec<_> = v.iter().filter(|x| x.ship_code == "ST" && x.sail_date == NaiveDate::from_ymd_opt(2027,1,2).unwrap()).collect(); assert_eq!(st.len(), 2); assert!(st.iter().any(|x| x.voyage_code == "4702" && x.nights == 7)); assert!(st.iter().any(|x| x.voyage_code == "4702A" && x.nights == 14)); }
#[test] fn schedule_currency_from_ship_currency() { let v = parse_schedule(&fixture("schedule_sample.json")); assert!(v.iter().any(|x| x.currency == "AUD")); assert!(v.iter().all(|x| !x.currency.is_empty())); }
```

- [ ] **Step 2: Run `nix develop -c cargo test -p royaltracker-api --test princess_schedule`.** Expected: FAIL.
- [ ] **Step 3: Implement it.**
- [ ] **Step 4: Run it (PASS), then `nix develop -c cargo test -p royaltracker-web --features sqlite` (PASS).**
- [ ] **Step 5: Commit** `public: Princess schedule and voyage-aware sailings endpoint`.

### Task 14: Public prices, subscribe and sweep for Princess voyages

**Files:**
- Modify: `crates/royaltracker-web/src/public.rs` (`prices`, `subscribe`, `list_subscriptions` DTO)
- Modify: `crates/royaltracker-core/src/public_sweep.rs`, `bins/royaltracker-scraper/src/main.rs`

**Interfaces:**
- Consumes: Task 11 `PrincessPriceCache`, Task 13 schedule, Task 3 `SailingKey`.
- Produces:
  - **Prices.** `GET /public/prices` gains an optional `voyage` param.
    - `brand=princess` without `voyage` → 400 `"voyage required for princess"`.
    - Princess lookups use `SailingKey { voyage_code: Some(voyage) }`. On a live fetch, use `PrincessPriceCache::products(&s.princess, voyage, currency)`, with currency from the cached schedule entry (default `"USD"`). Seed tracked products exactly as the RCG path does.
    - Snapshot: `promo_present = promo_cents.is_some()`, `adult_promo_price = promo_dollars()`, `child_promo_price = child_cents/100`.
  - **Subscribe.** `SubscribeReq` gains `voyage: Option<String>`, with the same 400 rule. Upsert the sailing with currency.
  - **Manage.** `PublicSubscriptionRow` (from Task 3) exposes `voyage_code`.
  - **Sweep.**
    - `run_public_sweep(client, princess: &PrincessGuestClient, repo, notifier, cfg)`.
    - Groups by `(Brand, ship, date, voyage_code)`.
    - Princess groups fetch via one `PrincessPriceCache` per sweep. An empty product map maps to the `PlannerNotOpen` branch.
    - The diff rule stays "promo present on both sides". Princess sets `promo_cents` whenever a price exists, so every Princess price change is diffable (spec §5.4).
    - The alert passes `currency` from the target.
  - `pub fn princess_fetch_outcome(products: HashMap<String, PublicProduct>) -> PublicFetch`. Empty → `PlannerNotOpen`, else `Products(values)`. It's a pure helper used by both web and sweep.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn princess_empty_catalog_is_planner_not_open() { assert!(matches!(princess_fetch_outcome(HashMap::new()), PublicFetch::PlannerNotOpen)); }
#[test] fn princess_nonempty_is_products() { let mut m = HashMap::new(); m.insert("2323".into(), prod("2323")); assert!(matches!(princess_fetch_outcome(m), PublicFetch::Products(v) if v.len() == 1)); }
#[test] fn princess_prices_require_voyage() { assert!(require_voyage(Brand::Princess, None).is_err()); assert!(require_voyage(Brand::Royal, None).is_ok()); assert_eq!(require_voyage(Brand::Princess, Some("4702A")).unwrap(), Some("4702A")); }
```

  `require_voyage(brand, voyage: Option<&str>) -> Result<Option<&str>, ApiErr>` is a helper in web `public.rs`.
- [ ] **Step 2: Run `TEST_PURE` and `nix develop -c cargo test -p royaltracker-web --features sqlite`.** Expected: FAIL.
- [ ] **Step 3: Implement it.**
- [ ] **Step 4: Run both test commands (PASS), then `BUILD_SQLITE` and `CHECK_PG` (PASS).**
- [ ] **Step 5: Commit** `public: Princess prices, subscriptions and sweep keyed by voyage`.

### Task 15: Public UI for Princess; Part C gate

**Files:**
- Modify: `crates/royaltracker-web/static/public/index.html`

**Interfaces:**
- Consumes: Tasks 13–14 HTTP.
- Behavior:
  - The brand `<select>` gains `<option value="princess">Princess</option>`.
  - The date picker label becomes "Sailing".
  - Options come from `SailingOption`:
    - with `voyage_code`: text `YYYY-MM-DD · N nights · Itinerary`, value = `voyage_code`;
    - otherwise: text and value are the date.
  - The selected option's `sail_date` and `voyage` travel with `/public/prices` and the subscribe body.
  - The footer disclaimer adds: "Princess prices shown are the standard rate; Captain Circle loyalty discounts aren't included."
  - Prices render the server's `price_label` (already currency-formatted).
  - "My alerts" rows show `voyage_code` when present.

- [ ] **Step 1: Manual check** via `nix develop -c cargo run -p royaltracker-web --example serve_public --features sqlite`. Open `/p`:
  1. pick Princess, Star Princess; expected: two entries on Jan 2 2027 (7 and 14 nights);
  2. pick the 14-night one; expected: prices load (> 200 excursions plus extras with per-day units);
  3. pick an AUD voyage; expected: labels read `AUD …`.
- [ ] **Step 2: Run `TEST_STORAGE && TEST_PURE && BUILD_SQLITE && CHECK_PG` and `nix develop -c cargo test -p royaltracker-web --features sqlite`.** Expected: all PASS.
- [ ] **Step 3: Commit** `public UI: Princess voyage picker`.
- [ ] **Step 4: Deploy checkpoint (ask the user).** Same flow as Task 7.
  - After rollout, check that `cruise_accounts` still isn't readable by the public role: `SET ROLE royaltracker_public; SELECT 1 FROM cruise_accounts;`. Expected: permission denied.
