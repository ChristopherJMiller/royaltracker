# Booked Add-ons Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Each 7am run reads the user's Royal Caribbean / Celebrity order history, works out what the cabin paid per item, tracks booked items automatically, shows "Booked $X/day · Save $Y if you rebook" on Tracking cards, and sends a "cheaper than you paid" alert.

**Architecture:** Phase A discovery also returns *order targets* (booking + this account's passenger id). Before the Phase A loop hands its clients to Phase B, each account reads order history for its targets. The reads are merged per reservation (all-or-nothing), summarized into `booked_items` rows, and synced onto `products_watched` (`alert_on_paid`). Phase B prices watches as today; `alert_on_paid` watches alert against the cabin's paid total, only for reservations whose orders were read fresh this run.

**Tech Stack:** Rust workspace (tokio, sqlx Postgres/SQLite via compile-time features, serde_json, teloxide), Preact + htm Mini App (`crates/royaltracker-web/static/app.js`), Helm/Argo deploy.

**Spec:** `docs/superpowers/specs/2026-11-06-booked-addons-design.md`. Read it before any task.

## Global Constraints

- Migrations are additive only, numbered `0015`, written for **both** `migrations/postgres` and `migrations/sqlite`. Do not add values to `alert_mode_kind`: SQLite enforces it with a CHECK constraint that can't be altered in place.
- No extra logins. Order reads use the clients Phase A already logged in with.
- Order-history calls: one summary + one detail per order, per booking, per linked account, with `pacing.min_interval` sleep between calls. Phase B pricing stays unpaced.
- Princess bookings are excluded everywhere.
- Never put guest names, reservation/passenger ids, order codes or cabin numbers in the repo (fixtures, tests, docs, commit messages). Tests use made-up values.
- An order read that fails, or a reservation not read fresh this run, never causes a below-paid alert and never removes booked items or tags.
- Copy (exact): tag `Booked $72.99/day`, `Booked $72.99/night`, `Booked $219.99`; unequal guests `Booked $109.49/day for 2`; status `Save $56 if you rebook` / `same price` / `you paid less`; alert line `below what you paid`.
- Telegram alert (exact, MarkdownV2-escaped by `escape_md_v2`):
  ```
  🔻 *{label} is cheaper than you paid*
  🚢 {itinerary}
  *{now}* {unit}{ for N} — you paid {paid}
  Rebook to save {savings} ({breakdown})      ← everyone paid the same; "(…)" omitted when no breakdown
  Rebook everyone to save {savings}           ← guests paid different amounts (" for N" shown above)
  ```

## Review Focus

1. **Per-day item on a booking with `nights` NULL or 0:** skip the item (no division by zero, no bogus price). Test in Task 3.
2. **Shared/group orders listing guests from another reservation:** only guests whose `reservationId` matches the booking count; a missing `reservationId` counts as own. Test in Task 3.
3. **One linked account's read fails while the other succeeds:** the reservation is not fresh, its booked items stay as they were, and no below-paid alert goes out. Test in Task 5.
4. **A 200 response without `payload.myOrders`:** treated as a failed read, not "nothing booked", so tags aren't wiped. Test in Task 2.
5. **Child guest on a product with no child price:** priced at the adult price. Test in Task 3.

---

### Task 1: Order targets from discovery, scrubbed order-history dump, real fixture

Recon first: confirm jdeath's field mapping against the user's real responses before anything relies on it. Steps 6–9 are run by the controller (kubectl access, PII review), not a subagent.

**Files:**
- Create: `crates/royaltracker-api/src/scrub.rs` (+ `pub mod scrub;` in `lib.rs`)
- Create: `crates/royaltracker-core/src/booked.rs` (only `OrderTarget` in this task; + `pub mod booked;` / re-export in `lib.rs`)
- Modify: `crates/royaltracker-core/src/bookings.rs` (`AccountDiscovery`, `discover_with_clients`)
- Modify: `bins/royaltracker-scraper/src/main.rs` (dump mode)
- Create (step 8): `crates/royaltracker-api/tests/fixtures/rcg/order_history.json`, `crates/royaltracker-api/tests/fixtures/rcg/order_detail_*.json`

**Interfaces:**
- Produces: `pub struct OrderTarget { pub brand: Brand, pub reservation_id: String, pub ship_code: String, pub sail_wire: String /* YYYYMMDD */, pub passenger_id: String }` in `royaltracker_core::booked`.
- Produces: `AccountDiscovery { report, rcg_clients, pub order_targets: Vec<OrderTarget> }`. `discover_with_clients` returns `(DiscoveryReport, HashMap<Brand, CruiseClient>, Vec<OrderTarget>)`.
- Produces: `pub fn scrub_json(v: &mut serde_json::Value)` in `royaltracker_api::scrub`.

- [ ] **Step 1: Write the failing scrubber test** in `scrub.rs`:

```rust
#[test]
fn scrubs_people_and_ids_but_keeps_products() {
    let mut v = json!({"payload": {"myOrders": [{"orderCode": "AB12CD", "orderDate": "2026-09-12"}],
      "orderHistoryDetailItems": [{"productSummary": {"id": "3222", "baseOptions": [{"selected": {"code": "3222"}}]},
        "guests": [{"id": "11112222", "firstName": "Pat", "lastName": "Doe", "reservationId": "1234567",
                    "stateroomNumber": "1234", "guestType": "ADULT", "sailingId": "QQ20301225"},
                   {"id": "11112222", "firstName": "Pat"}]}]}});
    scrub_json(&mut v);
    let g = &v["payload"]["orderHistoryDetailItems"][0]["guests"];
    assert_eq!(g[0]["firstName"], "REDACTED");
    assert_eq!(g[0]["lastName"], "REDACTED");
    assert_eq!(g[0]["stateroomNumber"], "REDACTED");
    assert_ne!(g[0]["id"], "11112222");
    assert_eq!(g[0]["id"], g[1]["id"]);               // same real id → same alias
    assert_ne!(g[0]["reservationId"], "1234567");
    assert_eq!(g[0]["sailingId"], "XX20990101");
    assert_ne!(v["payload"]["myOrders"][0]["orderCode"], "AB12CD");
    assert_eq!(g[0]["guestType"], "ADULT");
    assert_eq!(v["payload"]["orderHistoryDetailItems"][0]["productSummary"]["id"], "3222"); // product codes kept
}
```

- [ ] **Step 2: Run it, expect FAIL** (`scrub_json` not defined): `nix develop -c cargo test -p royaltracker-api --lib scrub`

- [ ] **Step 3: Implement `scrub_json`.** Recursive walk:
  - Replace with `"REDACTED"` the string values of `firstName`, `lastName`, `middleName`, `preferredName`, `fullName`, `email`, `emailAddress`, `phone`, `phoneNumber`, `birthDate`, `dateOfBirth`, `loyaltyNumber`, `stateroomNumber`, `addressLine1`, `addressLine2`.
  - Replace the values of `orderCode`, `reservationId`, `bookingId`, `passengerId`, `masterPassengerId`, `guestId`, and of `id` when the value is all digits with ≥ 6 digits, with consistent aliases `ID-1`, `ID-2`, … (same input → same alias).
  - Replace any string matching `^[A-Z]{2}\d{8}$` with `XX20990101`.

- [ ] **Step 4: Run it, expect PASS.**

- [ ] **Step 5: Collect order targets in discovery and add dump mode.**
  - In `discover_with_clients`, for each RCG summary that has `passenger_id` and `sail_wire` and hasn't sailed, push an `OrderTarget` (brand = the loop's brand). Return the vec as the third tuple element and carry it into `AccountDiscovery.order_targets`.
  - Web callers of `discover_account` ignore the new field.
  - In the scraper `main`, right after `Config::load()`: if env `ROYALTRACKER_DUMP_ORDERS` is set, run `dump_orders(&cfg, &repo, &cipher)` and return without the jitter sleep or any other phase.
  - `dump_orders`: for each active RCG account, call `discover_account(..., None)`. Then for each target, fetch `fetch_order_history` and every `fetch_order_detail` (codes from `myOrders` + `ordersOthersHaveBookedForMe`), `scrub_json` each, and print one JSON line per target: `{"brand","summary","details":[…]}`.
  - `cargo build -p royaltracker-scraper` must pass, and `cargo test --workspace` stays green.
  - Commit: `scraper: order targets from discovery, scrubbed order-history dump mode`.

- [ ] **Step 6 (controller): Ship the dump code.** PR → rebase-merge → wait for `image.yml` to push `royaltracker-scraper:<sha>`. Do not bump the homeops tag: the CronJob keeps the old image.

- [ ] **Step 7 (controller): Run the dump once as a one-off Job.** Not within 15 min of 07:00 ET, because of login rate limits.
  ```bash
  kubectl --context admin@galaxy -n royaltracker create job orders-dump-$(date +%s) --from=cronjob/royaltracker-scraper --dry-run=client -o yaml > $SCRATCH/dump-job.yaml
  # edit: container image → :<sha>; env add ROYALTRACKER_DUMP_ORDERS=1, ROYALTRACKER_JITTER_MINUTES=0
  kubectl --context admin@galaxy -n royaltracker apply -f $SCRATCH/dump-job.yaml
  kubectl --context admin@galaxy -n royaltracker wait --for=condition=complete --timeout=600s job/<name>
  kubectl --context admin@galaxy -n royaltracker logs job/<name> | grep '^{' > $SCRATCH/orders-dump.jsonl
  kubectl --context admin@galaxy -n royaltracker delete job/<name>
  ```

- [ ] **Step 8 (controller): Review by hand for PII, then write the fixtures.**
  - Read every line; any name, email, id, cabin or ship+date that survived gets replaced by hand.
  - Write `order_history.json` (one summary) and `order_detail_package.json` (a per-day package, ideally two guests). If present, also write `order_detail_oneoff.json` (an excursion/day pass) and `order_detail_cancelled.json` (has a `CANCELLED` guest).

- [ ] **Step 9 (controller): Record findings** in a `## Recon findings` section at the end of this plan:
  - the exact `salesUnit` strings and `guestType` values seen;
  - every `orderStatus` value seen;
  - whether `baseOptions[0].selected.code` ever differs from the catalog `product_code` of the same item (compare with `catalog_cache`);
  - whether `payload.myOrders` is present when empty;
  - where `guests[].reservationId` lives.

  Later tasks follow these findings wherever they contradict this plan. Commit the fixtures + findings: `test: scrubbed order-history fixtures and recon findings`.

---

### Task 2: Parse order history into order lines; variant-aware offering pick

**Files:**
- Create: `crates/royaltracker-api/src/orders.rs` (+ `mod orders; pub use orders::{…}` in `lib.rs`)
- Create: `crates/royaltracker-api/tests/orders.rs`
- Modify: `crates/royaltracker-types/src/lib.rs` (`PaidUnit`, `GuestType`)
- Modify: `crates/royaltracker-api/src/catalog.rs`, `client.rs` (`fetch_product_price` uses `ProductPrice::from_raw_for`)

**Interfaces:**
- Consumes: fixtures + findings from Task 1.
- Produces (types crate):
  - `pub enum PaidUnit { Day, Night, Each }`, with `as_str()` → `"day" | "night" | "each"` and `FromStr`.
  - `pub enum GuestType { Adult, Child }`.
- Produces (api crate):
  - `pub struct OrderRef { pub code: String, pub date: Option<NaiveDate> }`.
  - `pub struct OrderLine { pub order_code: String, pub order_date: Option<NaiveDate>, pub guest_id: String, pub guest_reservation_id: Option<String>, pub guest_type: GuestType, pub category_prefix: String, pub product_code: String, pub title: String, pub unit: PaidUnit, pub subtotal: f64, pub quantity: u32, pub currency: Option<String> }`.
  - `pub fn parse_order_summary(v: &Value) -> Result<Vec<OrderRef>, ApiError>`.
  - `pub fn parse_order_detail(order: &OrderRef, v: &Value) -> Vec<OrderLine>`.
  - `impl CruiseClient { pub async fn fetch_order_lines(&self, brand: Brand, ship_code: &str, sail_wire: &str, passenger_id: &str, reservation_id: &str, gap: Duration) -> Result<Vec<OrderLine>, ApiError> }`.
  - `ProductPrice::from_raw_for(raw: Value, requested_code: &str) -> ProductPrice`.

- [ ] **Step 1: Write failing tests** in `tests/orders.rs`, using the Task 1 fixtures plus small inline JSON:
  - `summary_lists_my_orders_and_others_skips_zero_totals`: the fixture yields ≥ 1 `OrderRef`. Inline JSON with an order whose `orderTotals.total` is `0` yields none for it; `ordersOthersHaveBookedForMe` entries are included; codes are de-duplicated.
  - `summary_without_my_orders_is_an_error`: `{"payload": {}}` → `Err`.
  - `package_detail_maps_fields`: from `order_detail_package.json`, every line has a non-empty `product_code` and `category_prefix`, `unit == PaidUnit::Day`, and `subtotal > 0`.
  - `cancelled_and_free_guests_skipped`: inline detail with three guests, statuses `CANCELLED` / subtotal `0` / normal → 1 line.
  - `variant_code_falls_back_to_default_variant`: no `baseOptions` → `product_code == productSummary.defaultVariantId`.
  - `sales_unit_mapping`: `PER_DAY`/`perDay` → `Day`, `PER_NIGHT`/`perNight` → `Night`, `perPackage` → `Each` (case-insensitive after removing `_`).
  - `guest_type_mapping`: `ADULT` → `Adult`; anything else → `Child`.
  - `offering_matches_requested_variant` (in `catalog.rs` tests): two offerings `"AAA1-XX20990101-A"` (promo 10) and `"AAA2-XX20990101-A"` (promo 20); `from_raw_for(raw, "AAA2").adult_price() == Some(20.0)`. With no matching id, behavior equals `from_raw`.

- [ ] **Step 2: Run, expect FAIL:** `nix develop -c cargo test -p royaltracker-api`

- [ ] **Step 3: Implement** the types, `parse_order_summary`, `parse_order_detail` and `from_raw_for`. Field paths are as in the spec's table (as corrected by the Recon findings).
  - `fetch_order_lines`: summary → for each `OrderRef`, sleep `gap` → `fetch_order_detail` → `parse_order_detail`.
  - Any HTTP or parse error returns `Err`. A partial read is never returned.

- [ ] **Step 4: Run, expect PASS** (whole api crate).

- [ ] **Step 5: Commit** `api: parse order history into order lines; price the booked variant`.

---

### Task 3: Summarize orders into booked items and compare with today

**Files:**
- Modify: `crates/royaltracker-types/src/lib.rs` (`BookedItem`)
- Modify: `crates/royaltracker-core/src/booked.rs`

**Interfaces:**
- Consumes: `OrderLine`, `PaidUnit`, `GuestType` (Task 2); `MIN_DROP_ABS`, `MIN_DROP_PCT` from `core::diff`.
- Produces (types): `pub struct BookedItem { pub reservation_id: String, pub category_prefix: String, pub product_code: String, pub title: String, pub unit: PaidUnit, pub adults: u32, pub children: u32, pub paid_total: f64, pub paid_each: Option<f64>, pub currency: Option<String>, pub last_alert_total: Option<f64>, pub orders_read_at: DateTime<Utc> }`.
  - `paid_total` is the cabin's total per unit (per day for `Day`, per night for `Night`, whole for `Each`).
  - `paid_each` is `Some` when every guest paid the same (within $0.005).
- Produces (core::booked):
  - `pub fn summarize_orders(reservation_id: &str, nights: Option<i32>, lines: &[OrderLine], now: DateTime<Utc>) -> Vec<BookedItem>`
  - `pub fn today_total(item: &BookedItem, adult: Option<f64>, child: Option<f64>) -> Option<f64>`
  - `pub enum PaidStatus { Cheaper { savings: f64 }, Same, PaidLess }`
  - `pub fn paid_status(item: &BookedItem, today_total: f64, nights: Option<i32>) -> PaidStatus`
  - `pub fn should_alert_below_paid(item: &BookedItem, today_total: f64) -> bool`
  - `pub fn paid_label(item: &BookedItem, currency: Option<&str>) -> String`
  - `pub fn savings_breakdown(item: &BookedItem, today_total: f64, nights: Option<i32>) -> Option<String>`

- [ ] **Step 1: Write failing unit tests** in `booked.rs`. Use made-up ids and the spec's numbers. Helper `line(guest, res, ty, unit, subtotal, qty, date)`.
  - `per_day_two_guests_same_price`: two adults, `Day`, subtotal 510.93 each, nights 7 → one item with `paid_total == 145.98`, `paid_each == Some(72.99)`, `adults == 2`.
  - `latest_order_wins_per_guest`: one guest, orders dated 2026-09-12 (510.93) and 2026-11-02 (482.93) → `paid_total == 68.99`.
  - `other_reservation_guests_ignored`: a line with `guest_reservation_id == Some("OTHER")` is dropped; `None` counts as own.
  - `per_day_without_nights_skipped`: nights `None` → no item; nights `Some(0)` → no item.
  - `quantity_divides_each_price`: `Each`, subtotal 40, quantity 2 → `paid_total == 20.0`.
  - `child_priced_at_adult_when_no_child_price`: 2 adults + 1 child, `today_total(item, Some(68.99), None) == Some(206.97)`.
  - `cheaper_same_paid_less`: `Day` item paid 145.98, nights 7. Today 137.98 → `Cheaper { savings: 56.0 }`; today 145.98 → `Same`; today 150.0 → `PaidLess`.
  - `pair_deal_is_not_cheaper`: guests paid 72.99 and 36.50 → `paid_total == 109.49`, `paid_each == None`. Today 137.98 → `PaidLess`, and `should_alert_below_paid == false`.
  - `alert_rules`: paid 145.98.
    - Last `None`, today 137.98 → true; today 145.50 (under $1 below) → false.
    - Last `Some(137.98)`, today 137.50 → false; today 130.00 → true.
  - `labels`:
    - equal `Day` → `"$72.99/day"`; `Night` → `"$41.99/night"`;
    - unequal `Day` 2 guests → `"$109.49/day for 2"`;
    - `Each` 1 guest → `"$219.99"`; `Each` unequal 2 guests → `"$439.98 for 2"`.
  - `breakdown`:
    - equal `Day`, 2 guests, nights 7, today 137.98 → `Some("2 guests × 7 days × $4.00")`;
    - `Night` 1 guest → `Some("7 nights × $3.00")`;
    - `Each` 1 guest → `None`; unequal → `None`.

- [ ] **Step 2: Run, expect FAIL:** `nix develop -c cargo test -p royaltracker-core booked`

- [ ] **Step 3: Implement.**
  - Per guest + product, pick the latest line by `(order_date, order_code)`.
  - Per-guest per-unit price = `subtotal ÷ nights` (Day/Night only) `÷ quantity` (when > 0), rounded to cents.
  - `should_alert_below_paid`:
    - With no `last_alert_total`: true when `paid_total − today ≥ MIN_DROP_ABS`.
    - Otherwise: true when the drop below `last_alert_total` meets both `MIN_DROP_ABS` and `MIN_DROP_PCT`.
  - Money formatting via `format_money`.

- [ ] **Step 4: Run, expect PASS.**

- [ ] **Step 5: Commit** `core: summarize orders into booked items; compare with today's price`.

---

### Task 4: Storage for booked items, paid alerts and dismissals

**Files:**
- Create: `migrations/postgres/0015_booked_items.sql`, `migrations/sqlite/0015_booked_items.sql`
- Modify: `crates/royaltracker-storage/src/repo.rs`, `postgres.rs`, `sqlite.rs`
- Modify: `crates/royaltracker-types/src/lib.rs` (`WatchedProduct.alert_on_paid: bool`) and every `WatchedProduct { … }` literal
- Create: `crates/royaltracker-storage/tests/booked.rs` (`#![cfg(feature = "sqlite")]`)

**Interfaces:**
- Consumes: `BookedItem`, `PaidUnit` (Tasks 2–3).
- Produces (repo trait):
  - `async fn replace_booked_items(&self, reservation_id: &str, items: &[BookedItem]) -> Result<BookedChanges, StorageError>`
    - `pub struct BookedChanges { pub added: Vec<BookedItem>, pub removed: Vec<String> }` (`removed` = product codes).
    - One transaction: upsert each item; delete rows not in `items`.
    - A row whose `paid_total` changed by > $0.005 gets `last_alert_total = NULL`; otherwise `last_alert_total` is kept.
  - `async fn list_booked_items(&self, reservation_id: &str) -> Result<Vec<BookedItem>, StorageError>`
  - `async fn set_booked_last_alert(&self, reservation_id: &str, product_code: &str, total: f64) -> Result<(), StorageError>`
  - `async fn ensure_booked_watch(&self, reservation_id: &str, category_prefix: &str, product_code: &str, label: &str) -> Result<Option<i64>, StorageError>`
    - No row → insert active with `alert_on_paid = true`.
    - `dismissed_at` set → `None`, untouched.
    - Inactive and not dismissed → reactivate, `alert_on_paid = true`.
    - Active → `alert_on_paid = true`.
  - `async fn release_booked_watch(&self, reservation_id: &str, product_code: &str) -> Result<(), StorageError>`
    - Only if `alert_on_paid`: set it false, `alert_mode = 'any_drop'`, `alert_threshold = NULL`.
  - `async fn set_watch_alert_on_paid(&self, watched_id: i64) -> Result<(), StorageError>`
  - Changed: `deactivate_watched` also sets `dismissed_at = now`; `upsert_watched` clears `dismissed_at`; `set_watch_alert` also sets `alert_on_paid = false`; `list_active_watched` fills `alert_on_paid`.

- [ ] **Step 1: Write the migrations.** Postgres:

```sql
-- What each booking's cabin has bought, from Cruise Planner order history
-- (paid_total = cabin total per unit). Rewritten by the daily scraper.
CREATE TABLE booked_items (
    reservation_id   TEXT NOT NULL REFERENCES bookings(reservation_id) ON DELETE CASCADE,
    product_code     TEXT NOT NULL,
    category_prefix  TEXT NOT NULL,
    title            TEXT NOT NULL,
    unit             TEXT NOT NULL CHECK (unit IN ('day', 'night', 'each')),
    adults           INTEGER NOT NULL,
    children         INTEGER NOT NULL,
    paid_total       DOUBLE PRECISION NOT NULL,
    paid_each        DOUBLE PRECISION,
    currency         TEXT,
    last_alert_total DOUBLE PRECISION,
    orders_read_at   TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (reservation_id, product_code)
);
ALTER TABLE products_watched ADD COLUMN alert_on_paid BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE products_watched ADD COLUMN dismissed_at TIMESTAMPTZ;
```

  SQLite: the same with `REAL`, `INTEGER NOT NULL DEFAULT 0` for the boolean, and `TEXT` timestamps (match `0013`).

- [ ] **Step 2: Write failing tests** in `tests/booked.rs`. Use a file-backed SQLite DB under `std::env::temp_dir()`, so a second raw `sqlx` pool can set up legacy state.
  - `replace_reports_added_and_removed`: first replace with A, B → `added == [A, B]`; second with B, C → `added == [C]`, `removed == ["A"]`.
  - `changed_paid_total_resets_last_alert`: set last alert on B; replace with B at the same total → kept; B at a new total → `None`.
  - `ensure_creates_tracked_watch`: → `Some(id)`; `list_active_watched` shows it with `alert_on_paid`.
  - `dismissed_watch_not_readded`: `upsert_watched` + `deactivate_watched` → `ensure_booked_watch == None`, still inactive.
  - `pre_launch_inactive_watch_comes_back`: `upsert_watched`, then raw `UPDATE products_watched SET active = 0` → `ensure_booked_watch == Some(id)`, active.
  - `readding_after_dismissal_clears_it`: dismiss, then `upsert_watched` → `ensure_booked_watch == Some(_)`.
  - `release_returns_to_any_drop`: from `alert_on_paid` → `alert_on_paid == false`, `alert_mode == AnyDrop`, threshold `None`.
  - `manual_alert_clears_paid`: `set_watch_alert(BelowThreshold, 68.0)` on an `alert_on_paid` watch → `alert_on_paid == false`.

- [ ] **Step 3: Run, expect FAIL:** `nix develop -c cargo test -p royaltracker-storage --features sqlite --test booked`

- [ ] **Step 4: Implement** the trait methods in both `sqlite.rs` and `postgres.rs`, and fix every `WatchedProduct` literal.

- [ ] **Step 5: Run, expect PASS.** Also run `nix develop -c cargo test --workspace` and `nix develop -c cargo check -p royaltracker-scraper` (postgres feature).

- [ ] **Step 6: Commit** `storage: booked items, paid alerts and watch dismissals (0015)`.

---

### Task 5: Read orders each morning and sync booked items onto watches

**Files:**
- Modify: `crates/royaltracker-core/src/booked.rs`
- Modify: `bins/royaltracker-scraper/src/main.rs`

**Interfaces:**
- Consumes: `OrderTarget`, `AccountDiscovery.order_targets` (Task 1); `fetch_order_lines` (Task 2); `summarize_orders` (Task 3); repo methods (Task 4).
- Produces (core::booked):
  - `pub struct OrderRead { pub reservation_id: String, pub lines: Result<Vec<OrderLine>, String> }`
  - `pub async fn read_account_orders(clients: &HashMap<Brand, CruiseClient>, targets: &[OrderTarget], gap: Duration) -> Vec<OrderRead>`
    - A target whose brand has no client yields `Err("no client")`.
  - `pub fn merge_reads(reads: Vec<OrderRead>) -> HashMap<String, Vec<OrderLine>>`
    - Only reservations where **every** read succeeded.
    - Lines de-duplicated on `(order_code, guest_id, product_code)`.
  - `pub async fn apply_order_reads(repo: &dyn PriceRepo, bookings: &HashMap<String, Booking>, reads: Vec<OrderRead>) -> HashSet<String>`
    - Returns the fresh reservations.
    - Per fresh reservation: `summarize_orders`, then `replace_booked_items`, then `ensure_booked_watch(…, &item.title)` for each `added`, and `release_booked_watch` for each `removed`.
    - Storage errors are logged and make that reservation not fresh.

- [ ] **Step 1: Write failing tests** for `merge_reads`:
  - `merge_dedupes_lines_seen_by_both_accounts`: two `Ok` reads of R1 sharing one `(order, guest, product)` → that line once.
  - `one_failed_account_makes_reservation_stale`: R2 has one `Ok` and one `Err` → R2 absent.
  - `all_failed_is_stale`: R3 with only `Err` → absent.

- [ ] **Step 2: Run, expect FAIL.**

- [ ] **Step 3: Implement** the three functions. Then wire the scraper:
  - `discover_for_account` returns `AccountDiscovery`.
  - In the Phase A loop, call `read_account_orders(&d.rcg_clients, &d.order_targets, pacing(&cfg.pacing).min_interval)` **before** the clients move into `clients_by_brand`; accumulate the reads.
  - After the loop, load `bookings_by_res`, then `let fresh_orders = apply_order_reads(&repo, &bookings_by_res, reads).await;`, and log counts only (no ids).
  - Dump mode stays available and unchanged.

- [ ] **Step 4: Run, expect PASS**, plus `cargo test --workspace` and `cargo check -p royaltracker-scraper`.

- [ ] **Step 5: Commit** `scraper: read order history each morning and track booked items`.

---

### Task 6: Below-paid alerts in the scrape cycle, Telegram and web push

**Files:**
- Modify: `crates/royaltracker-core/src/scrape.rs`
- Modify: `crates/royaltracker-telegram/src/lib.rs` (`PaidComparison`, `render_diff`)
- Modify: `crates/royaltracker-notify/src/lib.rs`, `webpush.rs`
- Modify: `bins/royaltracker-scraper/src/main.rs` (pass `&fresh_orders`)

**Interfaces:**
- Consumes: Task 3 functions; `list_booked_items`, `set_booked_last_alert` (Task 4); `fresh_orders` (Task 5).
- Produces (telegram, re-exported by notify):
  `pub struct PaidComparison<'a> { pub now_label: &'a str, pub paid_label: &'a str, pub unit: Option<&'a str> /* "per day" | "per night" */, pub group: Option<u32>, pub savings_label: &'a str, pub breakdown: Option<&'a str> }`
  - `DiffContext.paid: Option<PaidComparison<'a>>`; `PriceDropAlert.paid: Option<PaidComparison<'a>>` (existing callers pass `None`).
  - `now_label` / `paid_label` are per person when everyone paid the same, cabin totals otherwise (`group = Some(n)`).
- Produces (core::scrape): `enum PaidDecision { Regular, Skip, Alert { total: f64 } }` and `fn paid_decision(w: &WatchedProduct, item: Option<&BookedItem>, fresh: bool, current: &PriceSnapshot) -> PaidDecision`.
  - `run_scrape_cycle` gains a final parameter `fresh_orders: &HashSet<String>`.

- [ ] **Step 1: Write failing tests.**
  - Telegram `paid_message_equal_guests` (exact):
    `"🔻 *Deluxe Beverage Package is cheaper than you paid*\n🚢 Harmony of the Seas · 7 nights · Mar 14, 2027\n*$68\\.99* per day — you paid $72\\.99\nRebook to save $56\\.00 \\(2 guests × 7 days × $4\\.00\\)"`.
  - Telegram `paid_message_unequal_guests`:
    `"…\n*$137\\.98* per day for 2 — you paid $145\\.98\nRebook everyone to save $56\\.00"`.
  - Telegram: the existing `rcg_full_message_byte_identical` test still passes with `paid: None`.
  - Web push `paid_push_body`: title `"🔻 Deluxe Beverage Package is cheaper than you paid"`, body `"$68.99 per day — you paid $72.99\nRebook to save $56.00\nHarmony of the Seas · 7 nights · Mar 14, 2027"`.
  - Scrape `paid_decision_rules`:
    - not `alert_on_paid` → `Regular`;
    - `alert_on_paid` with no item → `Regular`;
    - item but `fresh == false` → `Skip`;
    - item fresh, snapshot adult 68.99 vs paid 145.98 for 2 adults → `Alert { total: 137.98 }`;
    - same with `last_alert_total = Some(137.98)` → `Skip`;
    - snapshot with `adult_promo_price: None` → `Skip`.

- [ ] **Step 2: Run, expect FAIL:** `nix develop -c cargo test -p royaltracker-telegram -p royaltracker-notify -p royaltracker-core`

- [ ] **Step 3: Implement.**
  - In `run_scrape_cycle`, load booked items per reservation once (cached like `catalog_index`). After the snapshot insert, branch on `paid_decision`:
    - `Regular` → today's `detect_diff` path.
    - `Skip` → `continue`.
    - `Alert` → `insert_diff(Diff::from_prices(w.id, item.paid_total, total))`, build `PaidComparison` (labels via `format_money` / `paid_label` / `savings_breakdown`), fan out as today, then `set_booked_last_alert(res, code, total)` when any send succeeded or there are no subscribers.
  - Pass `&fresh_orders` from main.

- [ ] **Step 4: Run, expect PASS**, plus `cargo test --workspace`.

- [ ] **Step 5: Commit** `alerts: tell you when a booked add-on is cheaper than you paid`.

---

### Task 7: Booked tag, savings and "below what you paid" in the Mini App

**Files:**
- Modify: `crates/royaltracker-web/src/handlers.rs` (`WatchedDto`, `list_watched`, `set_alert`)
- Modify: `crates/royaltracker-web/static/app.js` (`WatchedList`, `alertSummary`, `AlertEditor`)

**Interfaces:**
- Consumes: `list_booked_items`, `set_watch_alert_on_paid` (Task 4); `today_total`, `paid_status`, `paid_label` (Task 3).
- Produces: `WatchedDto.booked: Option<BookedDto>`, with `#[derive(Serialize)] pub struct BookedDto { pub paid_label: String, pub status: &'static str /* "cheaper" | "same" | "paid_less" | "unknown" */, pub savings_label: Option<String>, pub orders_read_at: DateTime<Utc> }`.
  - `WatchedDto.alert_mode` is `"below_paid"` when `alert_on_paid`.
  - `POST` alert body accepts `alert_mode: "below_paid"`: `400 "nothing booked for this item"` unless a booked item exists for the watch's reservation + product.

- [ ] **Step 1: Write failing unit tests** in the existing `#[cfg(test)] mod tests` of `handlers.rs` (the crate only has pure-function tests) for a pure `fn booked_dto(item: &BookedItem, snap: Option<&PriceSnapshot>, nights: Option<i32>, currency: Option<&str>) -> BookedDto`:
  - `cheaper_dto`: `paid_label == "$72.99/day"`, `status == "cheaper"`, `savings_label == Some("$56")` (whole dollars when `.00`, else cents).
  - `no_snapshot_is_unknown`: `status == "unknown"`, `savings_label == None`.
  - `same_and_paid_less`.

- [ ] **Step 2: Run, expect FAIL**, then implement `booked_dto`, `list_watched` (one `list_booked_items` per reservation; `nights` from the user's bookings) and `set_alert`.

- [ ] **Step 3: Mini App.**
  - On a `WatchedList` card with `w.booked`, add a row under the title row: an outlined pill `Booked ${paid_label}`, then:
    - `cheaper`: a highlighted pill `Save ${savings_label} if you rebook` (`bg-blue-100 text-blue-900`);
    - `same`: muted text `same price`;
    - `paid_less`: muted text `you paid less`;
    - `unknown`: nothing.
  - `alertSummary`: `below_paid` → `🔔 below what you paid`.
  - `AlertEditor`: when `watched.booked`, a third radio `Below what you paid` (value `below_paid`); saving it posts `{alert_mode: "below_paid"}`.
  - Layout matches board B of the mockup.

- [ ] **Step 4: Run tests, expect PASS.** Run `node --check crates/royaltracker-web/static/app.js` (via `nix-shell -p nodejs`) to catch syntax errors.

- [ ] **Step 5: Commit** `mini-app: booked tag, savings and "below what you paid" on tracked items`.

---

### Task 8 (controller): Verify, ship, first run

- [ ] **Step 1:**
  - `nix develop -c cargo test --workspace`
  - `nix develop -c cargo test -p royaltracker-storage --features sqlite`
  - clippy on the postgres build (as in PR #6): no new warnings.
- [ ] **Step 2: Migration rehearsal.** Scratch PG16 (`nix-shell -p postgresql_16`) loaded from a `pg_dump` of prod → apply `0015` with `sqlx migrate run --source migrations/postgres` → `\d booked_items`, row counts unchanged.
- [ ] **Step 3: Before deploy:** fresh in-cluster logical backup; PII scan of the full branch diff (`git diff main… | grep` for names/ids from the dump).
- [ ] **Step 4: Ship.** PR → rebase-merge → image → bump `image.tag` in luma-homeops → push → watch the rollout.
- [ ] **Step 5: First run.** Run one scraper Job by hand (`create job --from=cronjob/…`), not within 15 min of 07:00 ET, and tell the user first: real alerts may go out. Then check:
  - logs show order reads and fresh counts;
  - `booked_items` rows for the upcoming bookings;
  - watches with `alert_on_paid`;
  - the Mini App shows the tags.
- [ ] **Step 6:** Update memory (`project_booked_addons`) with what shipped and any surprises.

## Recon findings


Captured 2026-11-06 from both linked accounts × both upcoming RCG bookings (fixtures under `crates/royaltracker-api/tests/fixtures/rcg/`). Task 2 follows these over the spec's table:

- **Status, not money, says cancelled.** `myOrders[].status`, the detail's `payload.status`, each item's `status` and each guest's `orderStatus` are `BOOKED` or `CANCELLED`. A cancelled guest keeps its old `priceDetails.subtotal` (517.93) with `total` 0 and `quantity` 0, and a cancelled order keeps `orderTotals.total` (1222.2). So: skip summary orders with `status == "CANCELLED"` (saves the detail call) or `total == 0`; skip items with `status == "CANCELLED"`; skip guests with `orderStatus == "CANCELLED"` or `subtotal == 0` (Celebrity "Always Included" items are $0).
- **Units:** `salesUnit` seen: `PER_DAY`, `PER_NIGHT`, `PER_SEAT` (→ `Each`).
- **Guests:** `guestType` `ADULT` (also `ageGroup.code`); no children in the data. `guests[].reservationId` is present. Guest records also carry `dob` and `age` (now scrubbed).
- **Quantity:** use the guest's `priceDetails.quantity` (1 when booked); the item-level `quantity` is -1.
- **Variants differ from catalog codes.** `id == baseOptions[0].selected.code == defaultVariantId` for every item, but the booked VOOM is `33S2` (2 devices) while the catalog (and the user's watch) has only `33S1`. So variant handling is needed: `33S2` gets its own card and is priced by its own code.
- **Categories:** `productTypeCategory.id` is `pt_beverage` / `pt_internet` / `pt_shoreX`; catalog/watch ids are `beverage` / `internet` / `shorex`. Normalize: strip `pt_`, lowercase.
- **Both summary keys** are present even when empty.
- **Per-account reads are needed:** on the Royal booking one account has 5 `myOrders`; the other has 0 `myOrders` and 4 `ordersOthersHaveBookedForMe`.
- Sanity: Deluxe Beverage 482.93 ÷ 7 nights = $68.99/day (already rebooked at today's price); VOOM 2-device 265.93 ÷ 7 = $37.99/night.
