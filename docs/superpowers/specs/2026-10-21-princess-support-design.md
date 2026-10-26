# Princess Cruises support — design

Status: approved in conversation 2026-10-21, pending final doc review.
Scope: royaltracker (Telegram bot + mini-app + scraper + no-login public tier).

## 1. Intent

royaltracker watches Cruise Planner add-on prices (excursions, drinks, Wi-Fi, extras) and alerts on drops. Today it only speaks Royal Caribbean Group (RCG): Royal Caribbean and Celebrity. The goal is to add **Princess Cruises** as a third line, at the same level as the others:

- **Logged in (Telegram):** a person attaches a Princess account. Their upcoming Princess trips show up next to their Royal and Celebrity trips. They can watch products and get drop alerts, shared with anyone else on the same booking, just like Royal today.
- **No login (public tier):** anyone can pick Princess, then a ship, then a voyage, see current prices, and subscribe to drop alerts (Web Push), just like the public Royal and Celebrity lookup.

To support this, the "royaltracker account" concept is generalized. A **user** (the person, identified by Telegram) owns **cruise accounts**, one per cruise line. The user is the hub of all their cruise logins.

Why the shape is what it is: Princess's add-on prices turned out to be public per voyage (validated live, §9). The Princess login is needed to *find your trips*, not to *read prices*. That makes Princess cheaper and less bot-sensitive to scrape than Royal, and lets it join the public tier almost for free.

## 2. Goals

1. A user can hold at most one cruise account per cruise line. The two lines are:
   - **Royal Caribbean & Celebrity** (`rcg`): one RCG login covers both brands; the scraper already checks both.
   - **Princess** (`princess`).
2. Adding an account: pick the line, enter login ID/email and password. That line's client logs in, discovers bookings, and saves them under that account's user. All bookings from all of a user's accounts appear together, whichever account was added first.
3. The existing Royal/Celebrity users keep working with **zero action** after deploy. Each existing `users` row becomes a user plus one `rcg` cruise account carrying its current credentials. No Royal-vs-Celebrity guessing is needed: the account is RCG, and brand stays per booking.
4. Princess bookings have the same capabilities as Royal ones in the mini-app:
   - show the trip;
   - browse its catalog;
   - watch products with the existing alert modes;
   - price history and staleness;
   - shared bookings via `booking_subscribers`, with watches deduped per `(booking, product)` and alerts fanned out to every subscriber.
5. Princess in the public tier: ship → voyage picker → prices → subscribe, with one price fetch per voyage serving every watcher.
6. Fold in the fixes this change touches (§6.7).

## 3. Non-goals (this round)

- Signing into the tracker outside Telegram (a website login is a later trajectory). Telegram remains the identity.
- Tier-specific (loyalty) prices. We track the price everyone on a booking sees (§5.4).
- Booking lookup by booking number + name + DOB (Princess's password-less "find my booking" form). It is the fallback only if multi-booking accounts need it (§8, open item 1).
- Changing how RCG prices are fetched. The scraper still reuses one RCG login for all RCG bookings. The "last discoverer's passenger wins" behavior on shared bookings stays as-is.
- Princess cabin insights: deck/height/motion panel, deck-plan images, GTY cabin reveal. These are RCG-specific today. A Princess booking simply doesn't show that panel.
- Other Carnival brands on the same platform (the Princess API takes a `BookingCompany` header, e.g. Holland America). Not now, but don't paint the client into a Princess-only corner where it's free not to.

## 4. Anti-goals (failure even if it "works")

- Existing users have to re-register, or lose bookings, watches, or history in the migration.
- One user can see another user's accounts, bookings, catalog, or history. The aggregation risk *within* one user is accepted. Across users it is not.
- Storing Princess PII we don't need. The booking summary returns every passenger's name, DOB, address and email. Persist only what tracking needs.
- Hammering Princess, or tripping its rate limits or bot manager.
  - Princess: one login attempt per account per scrape run, with no automatic retry. A failure is recorded on the account (last login outcome) and surfaced to the user, like RCG failures today.
  - RCG: login behavior is unchanged, one attempt per brand host per run.
  - No cross-run backoff this round (my call; there's no such backoff in the code today).
- Logging secrets. The Princess login uses HTTP Basic with the user's password; that header must never reach logs.

## 5. Decided behavior

### 5.1 Identity and accounts

- `users` keeps being the person: Telegram chat id, username, active.
- New **cruise account** entity:
  - fields: owning user, cruise line (`rcg` | `princess`), login id, encrypted password (existing `royaltracker-crypto` scheme), active, and last login outcome (time plus error);
  - unique per (user, line);
  - adding an account for a line the user already has replaces that account's credentials.
- Bookings are subscribed to the *user* (existing `booking_subscribers`). Everything downstream of `user_id` stays intact.
- Removing a cruise account stops discovery for it. What happens to bookings only that account surfaced is the builder's call; the simplest is unsubscribing the user from them.
- `brand_pref` is legacy and unused (web registration always wrote `royal`, and discovery ignores it). It goes away.

### 5.2 Telegram bot and mini-app UX

- **Bot `/register`:** first choose the line with buttons ("Royal Caribbean & Celebrity" / "Princess"), then login id, then password. This replaces the current trailing brand step. Copy stops saying "Royal Caribbean / Celebrity" only.
- **Bot:** a way to list the user's attached accounts and remove one (e.g. `/accounts`). `/test` checks each attached account.
- **Mini-app:**
  - the register form gets the same line selector;
  - an accounts view lists attached accounts (line, login id, last login status) with add and remove;
  - bookings show a brand label: Royal Caribbean / Celebrity / Princess.

### 5.3 Princess booking discovery (logged in)

Per Princess account, each scrape run (and on `/register`, `/api/refresh`):

1. Log in.
2. Establish the booking-site session.
3. Read the current booking context: booking number, ship code, voyage code, passenger number, duration, departure.
4. Read the loyalty tier.

See §9.1 for the calls.

Persist each booking:

- `reservation_id` = booking number;
- `brand` = `princess`;
- `ship_code`, `sail_date`, `passenger_id` = passenger seq, `nights` = duration;
- voyage code (needed for pricing).

Subscribe the account's user to the booking. Link the booking to its `sailings` row.

- **Shared bookings:** a Princess booking number covers every passenger in the cabin. If a second user's own Princess account surfaces the same booking number, they're subscribed to the same booking and share watches and alerts. This matches RCG.
- **Not verified:** whether Princess lists a booking in a non-primary passenger's account. RCG does. Build for it; don't depend on it.
- **Accounts with several Princess bookings** (my call; see §8.1):
  - Phase 2 ships discovering the booking(s) the session exposes. Today that's the one booking the site opens on.
  - Structure discovery so a list of bookings can be plugged in later. Don't hard-code "exactly one".
  - If the account lands on no booking context (e.g. a picker page, or `sfccKeepAlive` returns no `bookingInfo`), record that on the account. Tell the user plainly: "Signed in to Princess, but couldn't open a booking: multi-booking accounts aren't supported yet." Never silently find zero trips.
  - Don't hold phase 2 on this.

### 5.4 Princess prices

- **Fetch:** prices come from a guest session (no login), with no shopper context set.
  - **Excursions** are fetched per **voyage**.
  - **Extras** (food-and-drinks, wifi, extras) are priced fleet-wide per **currency**: identical across every USD voyage tested. So they can be fetched once per currency per run and reused for every voyage in that currency.
  - Cache within a run. Every watch reuses the fetch: logged-in watches on any booking of that voyage, and public subscriptions.
- **Currency:** each voyage has a ship currency in the schedule (`shipCurrency`: 1,808 USD and 134 AUD in the snapshot).
  - Pass it as the search `currency`. AUD voyages are priced from AUD pricebooks (e.g. `pcl-extras-aud-sale`) and can carry a different extras set.
  - Store the currency with the price.
  - Show the currency code wherever it isn't USD, so alerts never show an AUD price as "$".
- **Categories:** `excursions`, `food-and-drinks`, `wifi`, `extras` (Princess's own category ids).
- **Tracked price:** the sellable price a guest sees, which is the search hit `price`.
- **Loyalty discounts are ignored for tracking.**
  - Platinum/Elite members get a fixed `wifi-loyalty` promotion: ~50% off Wi-Fi.
  - It is a static perk, not a sale that moves.
  - Tracking the common price keeps shared watches meaningful for everyone on a booking.
  - Capturing the loyalty price for display is optional polish, not a goal.
- **List price:** where Princess exposes one (a list pricebook, e.g. Wi-Fi "Book Early & Save 20%"), store it as the base price.
- **Change rule:** any change in a Princess product's sellable price is a real change and can alert.
  - This differs deliberately from the RCG public-sweep rule.
  - The RCG rule only diffs when a promo is present on both sides, because the RCG promo disappears when the sale window closes.
  - Most Princess products (e.g. excursions) have no separate promo/list split at all. Applying the RCG rule would mean they never alert.
- **Units:**
  - drinks and Wi-Fi are priced **per person per day** (`unit: "person / day"`);
  - excursions are per person (adult);
  - store and display the unit so alerts aren't misleading.
- **Child prices:** capture them where Princess exposes a child pricebook; adult is required.

### 5.5 Princess in the public tier

- Brand picker gains Princess, then a ship picker with Princess's 17 ships, then a **voyage** picker.
- **Princess voyages are identified by voyage code, not ship + date.** In the current schedule, 488 ship+date pairs have two voyages. Typically that's a 7-night trip and a 14-night back-to-back starting the same day, and each has its own code and its own catalog (e.g. `4702` vs `4702A`).
  - The picker lists each voyage as date · nights · itinerary name.
  - Prices and subscribe calls carry the voyage code.
- **Schedule source:** one unauthenticated call returns the whole schedule, about 1,900 voyages across all ships for two years (§9.3).
  - Refresh it at most daily (my call). Mechanism is the builder's: in-memory cache, or DB-backed `sailings` rows.
  - Royal/Celebrity public lookup is unchanged.
- Seed-on-lookup, subscriptions, Web Push delivery and the sweep keep their current behavior. The sweep just calls the Princess fetcher for Princess sailings.

### 5.6 Migration of existing data

- Every existing `users` row becomes one `rcg` cruise account (login id + encrypted password copied as-is) owned by that same user id. Bookings, subscribers, watches, snapshots and diffs are untouched.
- Postgres: adding `'princess'` to `brand_kind` must be its own migration file. `ALTER TYPE … ADD VALUE` can't be used in the same transaction that adds it, and sqlx runs each migration file in a transaction.
- SQLite: the brand CHECK constraints (`bookings`, `sailings`, `users.brand_pref`) need table rebuilds. `bookings` is an FK target of `products_watched`, `catalog_cache` and `booking_subscribers`, so rebuild carefully.
- **Rollback safety (my call):** don't drop the old credential columns on `users` in the same release that copies them into cruise accounts. Make them nullable and unused, and drop them in a later migration once the new image is proven. Argo auto-syncs, and an image rollback onto a schema missing those columns would break the bot.
- Both bot and scraper run `repo.migrate()` on startup (existing behavior). Next migration number is `0012`.

## 6. Constraints and repo facts the builder must respect

### 6.1 Existing architecture

- Rust workspace.
- DB backend chosen at compile time via mutually exclusive `sqlite` / `postgres` features on `royaltracker-storage`. Every storage change lands in both `sqlite.rs` and `postgres.rs`, and both `migrations/{sqlite,postgres}`.
- HTTP goes through `wreq` with Chrome TLS emulation. The `User-Agent` header must match the emulated browser: Akamai flags mismatches, which caused 403/404s in May 2026.
- The only traits are `PriceRepo` and `Notifier`. API clients are concrete structs (`CruiseClient`, `PublicClient`, `graphql.rs`). There's no cruise-line abstraction today; introducing one is the builder's call.

### 6.2 `Brand` today

`enum Brand { Royal, Celebrity }` in `royaltracker-types`.

- Adding `Princess` breaks 5 exhaustive matches:
  - `host`, `code`, `url_segment`, `as_str` (types/lib.rs);
  - the bot's display-name match.
- These will **silently do the wrong thing** for Princess:
  - `api_host()` ignores `self` and returns the RCCL host;
  - hardcoded `[Brand::Royal, Brand::Celebrity]` lists in `core/bookings.rs` and `api/client.rs`;
  - hardcoded RCCL hosts in `graphql.rs` and `api/public.rs`;
  - `WEB_APP_KEY`;
  - `FromStr`;
  - `BRAND_LABEL` in `app.js`;
  - the public `index.html` brand `<select>`;
  - the bot keyboard;
  - the PG enum and the SQLite CHECKs.
- Brand (Royal/Celebrity/Princess) and cruise line (`rcg`/`princess`) are different concepts. Brand is per booking/sailing; line is per account.

### 6.3 Ship codes collide across lines

Princess ship codes overlap existing ones:

- `EP` = Emerald Princess vs Explorer of the Seas;
- `SU` = Sun Princess vs Celebrity Summit.

Everything keyed by ship code alone must become brand-qualified:

- `ship_name()`, `all_ships()`, `cabin_deck_range()`;
- the `deck_plans` table PK `(ship_code, deck)`;
- `SHIP_NAMES` / `deckOf` / the cruisedeckplans slug in `static/app.js`;
- the deck-plan ownership check (`handlers.rs`, matches by ship code only).

### 6.4 Booking identity

- `bookings` PK is `reservation_id` alone, with FKs from `products_watched`, `catalog_cache` and `booking_subscribers`.
- A booking's identity is (cruise line, booking number). A booking discovered for one line must never overwrite another line's booking.
- **My call:**
  - keep `reservation_id` as the PK and enforce the line on upsert (refuse or skip a cross-line conflict, don't overwrite);
  - don't rewrite three FKs;
  - this is safe because the formats can't collide: RCG reservation ids are 7 digits, Princess booking numbers are 6 alphanumeric characters.
  - If the builder finds a Princess number that can be all-digit and 7 long, switch to a composite key.

### 6.5 `sailings`

- `sailings` is `UNIQUE (brand, ship_code, sail_date)`, and the public-tier series (`tracked_products`, `sailing_price_snapshots`, `sailing_diffs`, `public_subscriptions`) hang off it.
- Princess needs the voyage code in the sailing identity, since ship+date isn't unique.
- RCG sailings stay identified by ship+date. RCG voyage ids exist but the RCG price API doesn't need them.
- Schema specifics are the builder's, but existing RCG rows and their uniqueness must survive unchanged.

### 6.6 Scraper flow today

- `bins/royaltracker-scraper/src/main.rs`:
  1. public sweep first;
  2. then per-user discovery, building a `Brand → CruiseClient` map (first user wins);
  3. then `run_scrape_cycle` over active watches.
- New flow:
  - per-*account* discovery, with each account dispatched to its line's client;
  - Princess watches priced via the per-voyage guest fetch;
  - the RCG path is unchanged in behavior.
- The `users.is_empty()` gate becomes "no active accounts".

### 6.7 Fixes to fold in (my call; the user saw this list)

1. **Celebrity booking gets Royal's catalog.** `refresh_catalog` (`royaltracker-web/src/handlers.rs` ~176-183) builds the client with a placeholder `Brand::Royal`. `fetch_catalog` uses the client's brand for the GraphQL path segment, so Celebrity bookings get the Royal catalog. Use the booking's brand.
2. **New bookings never get `sailing_id`.** Only the 0010 backfill set it; `upsert_booking` doesn't. Link on every upsert. Princess needs this for per-voyage pricing anyway.
3. **Catalog search/browse aren't scoped to the caller.** `search_catalog` / `browse_catalog` (`handlers.rs` ~317-346) don't check that the caller is subscribed to the reservation, and search queries all of `catalog_cache`. Scope both to the caller's subscribed bookings.
4. **Brand-qualify ship-code lookups** (§6.3).

### 6.8 Public role and deploy

- `migrations/postgres/0011_public_role_grants.sql` grants the least-privilege `royaltracker_public` role access to public-tier tables only.
- Any new or altered public-tier table needs matching grants.
- `cruise_accounts` (credentials) must **not** be granted to that role.
- Deploy is the existing two-repo flow:
  1. royaltracker `main` → image build;
  2. bump `image.tag` in `luma-homeops`;
  3. Argo syncs.

### 6.9 Fixtures

- The repo is **public**. Test fixtures recorded from Princess responses must be scrubbed of names, DOBs, addresses, emails, loyalty numbers and booking numbers.
- Catalog/price/schedule responses are public data and fine to commit.

## 7. Left to the builder

- Whether to introduce a cruise-line trait over the RCG and Princess clients, or dispatch with a `match`.
- Exact schema for cruise accounts, the voyage code on sailings/bookings, and how the legacy `users` columns are retired (within §5.6's rollback rule).
- Where Princess catalog data lives for mini-app browsing (reusing `catalog_cache` per reservation is fine).
- Whether Princess logged-in watches stay on `products_watched` / `price_snapshots`, or converge on the public `tracked_products` series.
  - Either is fine if behavior matches §5.3–5.4.
  - The per-voyage fetch must be shared within a run.
- Schedule refresh mechanism and cache location (§5.5).
- Pagination and limits for the SFCC search (`limit=200` returned full categories in testing).
- Test strategy. Expected at minimum:
  - migration test (existing user → `rcg` account, data intact) on SQLite;
  - Princess response parsers tested against scrubbed fixtures;
  - both DB features compile;
  - env-gated live smoke tests like the existing `crates/royaltracker-api/tests/login_smoke.rs`.
- Phasing (my recommendation; each phase ships on its own):
  1. accounts model + migration + §6.7 fixes, with no Princess yet;
  2. Princess client + logged-in tracking;
  3. Princess in the public tier.

  No date pressure: the sailings driving this are far enough out that the order stands.

## 8. Open items / risks (unverified)

1. **Accounts with several Princess bookings.**
   - The test account has one booking, so `manage-booking/index.page` 302s straight into it.
   - The booking context is server-session state, and no booking-list endpoint was observed in the site's JS.
   - Phase 2 ships without solving this (§5.3).
   - If a multi-booking account becomes available, investigate how the site switches bookings: what `index.page` serves, and the My Princess "upcoming cruises" page.
   - Fallback, still a non-goal: the password-less manage-booking form (`book.princess.com/manage-booking/login.page`, fields first/last name, DOB, 6-char `bookingId`). It opens a session on a specific booking. If ever built, use the name and DOB once and don't persist them (§4).
2. **Non-primary passenger visibility** of a booking in their own account (§5.3).
3. **Voyage-specific extras.**
   - The site first fetches the voyage's sellable special-service list (`custom/ssv`, which needs booking headers incl. a 30-min `appToken`), then filters extras by those ids.
   - As a guest, the unfiltered category search returned the same set for the original voyage.
   - It also returned the same counts (33 food-and-drinks / 2 wifi / 9 extras) on every USD voyage tested. The AUD voyage had 1 wifi product.
   - So the unfiltered set may include items a specific voyage doesn't sell.
   - When a booking session exists (logged-in discovery), the list can be fetched.
4. **Rate limits / bot manager on Princess login.**
   - Plain `curl` (Firefox UA) logged in fine from a home IP with no challenge. Thresholds are unknown.
   - RCG's soft auth limit (~5 attempts per 10 min per IP; see the Phase 0 notes) suggests user-triggered bursts (`/register`, `/test`, `/api/refresh` in quick succession) are the realistic risk, not the daily run.
   - No backoff is required this round (§4). Add a cooldown only if bursts turn out to trip it.
5. **How often Princess prices actually move** is unknown. Excursions showed a single sale pricebook; Wi-Fi showed an early-booking promotion.

## 9. Princess API reference (validated 2026-10-18…21)

Common traits:

- **Hosts:** `gw.api.princess.com` (Princess gateway), `book.princess.com` (legacy booking site, cookie session), `www.princess.com/mobify/…` (Salesforce Commerce Cloud PWA Kit proxy).
- **Bot protection:** Akamai Bot Manager cookies (`_abck`, `bm_sz`, `ak_bmsc`) on all hosts. Plain HTTPS with a browser UA worked for every call below.
- **Common gateway headers:**
  - `BookingCompany: PC`
  - `ProductCompany: PC`
  - `ReqSrc: W`
  - `pcl-client-id: 32e7224ac6cc41302f673c5f5d27b4ba` (static, public, shipped in site JS)
  - `Origin: https://www.princess.com`

### 9.1 Login and booking context (needs account)

1. **Login.** `POST https://gw.api.princess.com/pcl-web/internal/guest/p1.0/login`
   - Empty body.
   - `Authorization: Basic base64(urlencode(login) + ":" + urlencode(password))`.
   - `AppId: {"env":"prod","migration":"N","sessionId":"<uuid4>"}`.
   - Returns `{"token": "<JWT, 30 min>"}`.
   - Sets cookies `pcl-ccntkn`, `pcl-ccnexp`, `JSESSIONID` (domain `.princess.com`).
   - JWT claims include `ccn` (Captain Circle no.), `uiid`, and scope `mail openid cn ocean creduser isMemberOf siebel`.
   - Optional precheck: `GET …/guest/p1.0/account-lookup?username=<login>` returns `{"scope":"ocean","status":"Active"}`.
2. **Bridge into the booking site.** `GET https://book.princess.com/captaincircle/quickLogin.do?temporary=<epoch_ms>` with the cookie jar returns `{"status":"success",…}`.
3. **Establish the current booking.** `GET https://book.princess.com/manage-booking/index.page` with the cookie jar, following redirects (it 302s through `/cruisepersonalizer/summary.page` to `/manage-booking/summary.page`).
   - This navigation sets the server session's current booking (`pcl_voyageId` / `pcl_shipCode` cookies); without it, `sfccKeepAlive` returns no `bookingInfo`.
4. **Current booking context.** `GET https://book.princess.com/manage-booking/json/sfccKeepAlive.page`, with headers `bookingcompany: PC` and `productcompany: PC`.
   - Returns `data.bookingInfo {bookingId, shipCode, voyageId, paxId, durationDays, departureDate ("Weekday, Month DD, YYYY" text), bookingCurrency, agencyId, bookCompany}`.
   - Also returns `data.tokenObj.appToken` (30 min).
   - `manage-booking/json/keepAlive.page` refreshes `appToken`.
5. **Gateway booking APIs.** Use headers `authtoken: <appToken>` and `appid: {"cruiseLineCode":"PCL","sessionId":"Unique Token","systemId":"PB","agencyId":"<agencyId>","bookNum":"<bookingId>"}`:
   - `GET …/cpmf/v1.0/booking/display/menu` returns `nav-menu {bookingId, shipCode, shipName, voyageCode, loyaltyLevel, itineraryName, daysToVoyage, durationDays, paxId}`.
   - `GET …/cpmf/v1.0/booking/summary` returns the full booking:
     - `paxList.paxes[] {seqNumber, pastPaxStatusName (tier), ageCode, …}` plus heavy PII (don't persist);
     - `specialServices.items[] {type, number, desc, paxIncluded}` (e.g. a Premier package);
     - pricing.
     - the cabin: `cabinNumber` (deck letter + number, e.g. `D123`), `deck` (e.g. `DOLPHIN`), `categoryName`. The account client reads these once per login, right after `sfccKeepAlive`, best-effort (added 2026-10-26).

### 9.2 Prices (no account; guest)

1. **Guest token.** `POST https://www.princess.com/mobify/slas/private/shopper/auth/v1/organizations/f_ecom_blnk_prd/oauth2/token`
   - `Authorization: Basic base64("25ddb611-56bf-4c76-85e1-c7c83c412085:_PLACEHOLDER_PROXY-PWA_KIT_SLAS_CLIENT_SECRET")`. The proxy injects the real secret; the literal placeholder is what the site sends.
   - `Content-Type: application/x-www-form-urlencoded`, body `grant_type=client_credentials&channel_id=PCL&dnt=false`.
   - Returns `{access_token (30 min), refresh_token (30 d), usid, customer_id}`. The token is a Guest (`isb … upn:Guest`).
2. **Excursions.** `GET https://www.princess.com/mobify/proxy/api/search/shopper-search/v1/organizations/f_ecom_blnk_prd/product-search` with `Authorization: Bearer <token>`.
   - Params:
     - `siteId=PCL`
     - `refine=cgid=excursions`
     - `refine=ilids=<voyageCode>`
     - `refine=htype=master|bundle|set`
     - `refine=price=(0..)`
     - `currency=USD`, `locale=en-US`
     - `expand=prices,promotions,represented_products`
     - `offset`, `limit` (≤200)
   - Returns `total` and `hits[]`. Each hit has:
     - `productId` (e.g. `SJD-100`, port-scoped, reused across voyages);
     - `productName`, `price`;
     - `c_priceRanges[] {pricebook ("pcl-usd-sale-adult"), minPrice, maxPrice}`;
     - `c_portId`, `c_portName`, `c_duration`;
     - `representedProduct.id` (voyage-specific variant, shape `<voyage>-SJD100A-<MMDDYYYY>`).
   - Stock: `GET …/product/shopper-availability/v1/organizations/f_ecom_blnk_prd/availability?siteId=PCL&productIds=a,b&inventoryIds=<voyageCode>` returns `ats` / `stockLevel`.
3. **Extras.** Same search with `refine=cgid=<food-and-drinks|wifi|extras>`, `refine=ilids=pcl-extras-inventory`, and optionally `refine=id=<pipe-joined ssv ids>`.
   - Hits carry `price` and, where present, `c_priceRanges` with `pcl-extras-usd-sale` and `pcl-extras-usd-list`.
   - Voyage ssv ids: `POST …/custom/ssv/v1/organizations/f_ecom_blnk_prd/products?siteId=PCL&c_voyageid=<voyageCode>` returns `{specialServiceCodes[], minDrinkingAge}`.
     - Needs headers `c_booknum`, `c_authtoken` (appToken), `c_pcl-client-id`, `c_agencyId`, `c_bookingCompany`.
     - As a guest it returns 400.
4. **Detail with promotions.** `GET …/product/shopper-products/v1/organizations/f_ecom_blnk_prd/products?siteId=PCL&ids=a,b&expand=prices,promotions&currency=USD` returns `price`, `unit` ("person / day"), `c_pricingPerDayRate`, `productPromotions[] {promotionId, promotionalPrice, calloutMsg}`.
5. **Personalization.** `PUT …/shopper/shopper-context/v1/organizations/f_ecom_blnk_prd/shopper-context/<usid>?siteId=PCL` with body `{"assignmentQualifiers":{"shipCode","currency"},"customQualifiers":{"shipCode","tradeCode","tier","voyageDuration","ssv_currentPax"}}`.
   - **Validated:** with no context, with the site's exact context, and with each tier, all 130 products (86 excursions + 44 extras) on a sample voyage priced identically. They also matched a logged-in browser session to the cent.
   - **Validated across voyages:** no context vs a voyage-matched context (`shipCode`, `tradeCode`, `voyageDuration`, `currency`) made no difference on any of six more voyages (108–244 products each):
     - a 14-night Caribbean back-to-back (ST);
     - a 13-night Australia/NZ trip in AUD (RP);
     - a 28-night trip (CO);
     - a 7-night California trip (EP);
     - a 10-night Alaska trip (RU);
     - a 7-night Mexico trip sailing within days (XP).
   - Extras were identical across all USD voyages. The AUD voyage used AUD pricebooks.
   - The sole exception: `tier` PLATINUM/ELITE adds promotion `wifi-loyalty` (12.50 vs 24.99/day; 20.00 vs 39.99/day), visible only in product detail promotions, not search `price`.
   - BLUE/GOLD/RUBY: no change. The special-service qualifier (e.g. a Premier package) changed nothing.
   - **We don't set context.**

### 9.3 Ships and schedule (no account)

- **Ships.** `GET https://gw.api.princess.com/pcl-web/internal/resdb/p1.0/ships` with the common gateway headers returns `{ships:[{id, name, versionDates[]}]}`. There are 17 ships:
  - AP Grand
  - CB Caribbean
  - CO Coral
  - DI Diamond
  - EP Emerald
  - EX Enchanted
  - GP Regal
  - IP Island
  - KP Crown
  - MJ Majestic
  - RP Royal
  - RU Ruby
  - SA Sapphire
  - ST Star
  - SU Sun
  - XP Discovery
  - YP Sky
- **Schedule.** `GET …/resdb/p1.0/products?agencyCountry=US&cruiseType=C&voyageStatus=A&webDisplay=Y&promoFilter=all&light=false`, about 850 KB.
  - The `ship=` filter is ignored; the whole schedule comes back.
  - Returns `products[] {id (itinerary product), name (itinerary), trades[], cruises[] {id (voyage code), voyage {id, ship {id, version}, sailDate (YYYYMMDD), duration, startPortId, endPortId}, startDate, endDate}}`.
  - Snapshot: 966 itinerary products, 1,943 voyages, Oct 2026–Nov 2028.
  - Voyage codes are unique; ship+date is not.
  - Back-to-back combo voyages have their own code and catalog (e.g. `4702` 7-night, 116 excursions; `4702A` 14-night, 268 excursions).
  - `light=true` omits voyage codes.

### 9.4 Identifiers

- Booking number: 6 alphanumeric characters, shared by all passengers in the cabin.
- Passengers: seq `01…NN` within the booking (gateway form `PC<booking><seq>`). Each passenger has their own Captain Circle number and tier.
- Voyage code: e.g. `X637`, `4702A`.
- Ship code: 2 characters.
