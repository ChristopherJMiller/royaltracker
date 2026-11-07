# Booked add-ons: compare today's price with what you paid

Date: 2026-11-06 · Status: draft

## Why

The user and their travel companion rebook Cruise Planner add-ons whenever the price drops below
what they paid. Today that is manual: they open Royal's order history page, divide the total by
the number of nights to get a per-day price, then set a "below $X" alert in the tracker by hand
(for example, a $68 threshold on a drink package that is $68.99/day). When an alert fires they
rebook right away.

The tracker should know what they have already booked, do the per-day math, and alert when
today's price beats what they paid, with no hand-set thresholds.

## What success looks like

- Every paid add-on on their Royal Caribbean and Celebrity bookings shows up in **Tracking** with
  a "Booked $72.99/day" tag, without them adding it.
- Each of those cards says whether today's price beats what they paid, and by how much in total.
- A Telegram alert arrives when today's price drops below what they paid.
- The morning after they rebook, the comparison uses the new, lower price. No false alert.

## Goals

1. Each 7am run reads order history for every Royal Caribbean and Celebrity booking, **before**
   prices are checked.
2. Work out what was paid per item: per day for per-day/per-night items, per unit otherwise.
3. Booked items are tracked automatically, alerting when today's price is below what was paid.
4. Show the booked tag and savings on the Tracking cards (layout B below).
5. A new alert message for "cheaper than you paid".

## Non-goals

- Princess bookings (their orders come from a different system).
- A separate "Booked" section. The user looked at one (layout A) and chose layout B.
- Refreshing orders more often than the 7am run. The user said the paid price updating "every
  7am cycle" is what they need; a mid-day rebook shows up the next morning.
- Rebooking on the user's behalf.
- The no-login public tier.

## Anti-goals (a failure even if it technically works)

- A false "cheaper than you paid" alert. In particular: the morning after a rebook, or because an
  order fetch failed and the paid price went missing or stale.
- Two cards for the same product on the same booking.
- Re-adding a card the user removed.
- Guest names, reservation numbers, passenger ids or cabin numbers landing in this public repo
  (fixtures, logs, commit messages).

## How it behaves (decided)

### The morning run

Today the scraper runs: public sweep → per-account login and booking discovery (Phase A) → price
scrape (Phase B). The order-history refresh goes **between Phase A and Phase B**, using the
authenticated clients Phase A already built (no extra logins). Phase B compares against the paid
prices from this same run.

**Read orders from every linked account.** *(My call.)* Order history is relative to the account
asking (`myOrders` = what that account bought, `ordersOthersHaveBookedForMe` = what others bought
for it), and both of the user's upcoming RCG bookings are shared by two accounts. So for each
booking, read order history once **per linked account**, each with that account's own client and
its own passenger id for that booking, and merge the results by order code + guest id. This needs
two things the code doesn't keep today: Phase A keeps only the first logged-in client per brand
(`clients_by_brand.entry(brand).or_insert`), and `upsert_booking` overwrites `bookings.passenger_id`
/ `account_id` with whichever account was discovered last. Keep every account's client from Phase
A and each account's passenger id per booking. If one account's read fails, use the other's and
keep the last known data for anything only the failed one could see.

### Reading orders

The API calls already exist in `crates/royaltracker-api/src/client.rs` (`fetch_order_history`,
`fetch_order_detail`; today they are only used to find GTY cabin numbers). The summary lists
`payload.myOrders` and `payload.ordersOthersHaveBookedForMe`; each order's detail lists
`payload.orderHistoryDetailItems`. Field mapping, from jdeath's maintained reference
(`scratch/jdeath/CheckRoyalCaribbeanPrice.py`, `getOrders`, updated for Royal's Feb 2026 variant
change):

| What | Where |
|---|---|
| skip whole order | `orderTotals.total` is 0 |
| product code | `productSummary.baseOptions[0].selected.code`, else `productSummary.defaultVariantId` |
| category prefix | `productSummary.productTypeCategory.id` |
| per-day item | `productSummary.salesUnit` is `PER_DAY` or `PER_NIGHT` |
| per guest | `guests[]`: skip `orderStatus == "CANCELLED"`; `priceDetails.subtotal` (skip 0), `priceDetails.quantity`, `priceDetails.currency`, `guestType` |

A guest's paid price = `subtotal` ÷ nights (per-day items only; nights from `bookings.nights`)
÷ `quantity` (when > 0). This is the same unit the scraper already stores for these products
(the Deluxe Beverage Package snapshot today is a per-day price).

**One order per guest and product: the latest one wins.** *(My call.)* If a guest has more than
one non-cancelled order for the same product (they bought the new one before cancelling the old,
or the old one still shows a pending/refunding status at 7am), only the most recent order counts.
Otherwise the morning after a rebook would compare against the old, higher price and send a false
alert.

**Variants: compare like with like.** *(My call.)* A booked item is identified by the variant
that was bought (`baseOptions[0].selected.code`), e.g. a 2-device Wi-Fi package, not the
catalog's default 1-device one. Its price check must fetch that variant's price (pick the matching
offering, not just the first with a promo price), so a cheaper default variant never triggers a
"cheaper than you paid" alert. It only folds onto an existing card when the codes match;
otherwise it gets its own card, labelled with the variant name. The fixture (see Testing) shows
whether the user's current bookings even differ; confirm before building variant handling.

### What "paid" a card shows

**Compare the cabin's total.** The user rebooks for everyone in the cabin at once, never one
person at a time. So for each booked product, compare what **everyone paid in total** with **today's
price for the same people** (per day for per-day items; each guest priced at their guest type's
price where the product has one). This stays honest when guests paid different amounts (bought on
different days, or a "second guest half off" deal). With A at $72.99/day, B at $36.50/day and
today $68.99/day each, the cabin paid $109.49/day against $137.98/day today: not cheaper, no
alert.

### The cards (layout B)

On the booking screen's Tracking list:

- Booked items appear automatically, next to the ones the user added themselves.
- Tag: "Booked $72.99/day" when everyone paid the same per person ("/night" for per-night items,
  whole price for one-off items); when they paid different amounts, the cabin total, e.g.
  "Booked $109.49/day for 2".
- Status next to the tag, from the cabin totals:
  - today < paid: "Save $56 if you rebook", highlighted. Savings = (cabin paid total − today's
    total for the same people), × nights for per-day items.
  - today = paid: "same price", muted.
  - today > paid: "you paid less", muted.
- Alert line: "below what you paid".
- Items the user tracks but hasn't booked look exactly as they do today.

Mockup (private to the user): https://claude.ai/artifact/T3Yk54hvagnL1c7sTakxs3, board "B ·
Folded into Tracking" and the alert board.

### Alerts

Fire when today's cabin total drops below what the cabin paid. Telegram text, in the style of
the current price-drop message (`royaltracker-telegram/src/lib.rs`, `render_diff`):

```
Deluxe Beverage Package is cheaper than you paid
Harmony of the Seas · 7 nights · Mar 14, 2027
$68.99 per day — you paid $72.99
Rebook to save $56.00 (2 guests × 7 days × $4.00)
```

When guests paid different amounts, the last two lines use cabin totals instead (e.g. "$137.98
per day for 2 — you paid $145.98" / "Rebook everyone to save $56.00").

When it fires again *(my call)*:
- Once when today's total first goes below what was paid.
- After a rebook, the paid price moves down; it fires again only when today's total goes below
  the **new** paid total.
- If they don't rebook and it keeps dropping, it fires again for each further drop below the last
  alerted total, using the existing noise floor (≥ $1 and ≥ 1%). Since they rebook right away,
  every further drop is another chance to save.

### Edge cases

- **Already tracking it:** if a booked product already has a watch (for example the $68
  hand-set drink-package alert), that card gets the tag and its alert switches to "below what
  you paid", replacing the hand-set threshold. *(My guess; the user did not object.)*
- **User removes a booked card:** the watch is deactivated as today, and the morning run must not
  recreate or reactivate it. Today `active = false` is the only signal, and `add_watched` /
  `upsert_watched` flip it back, so record a removal separately. *(My call:)* watches removed
  **before** this ships don't count as removals. A booked product with an old inactive watch
  comes back on the first run, because the user asked to see everything they've booked. Only
  removals after launch stick.
- **Cancelled without rebooking:** once a product has no paid, non-cancelled order left, the tag
  disappears, the card stays tracked, and its alert goes back to "any drop". *(My call.)*
- **Order fetch fails** (whole booking or a single order detail): keep the last known paid prices
  and tags; never treat a failure as "nothing booked"; never alert from missing data; log it.
- **$0 items** (perks, included packages): skipped.

## Constraints

- Rust workspace: Postgres in prod, SQLite in dev, selected at compile time; every migration in
  both `migrations/postgres` and `migrations/sqlite`, and additive only (adding a value to the
  `alert_mode_kind` enum counts as additive).
- The scraper is a daily Kubernetes CronJob at 07:00 America/New_York, plus 0–10 min jitter.
  Today the personal price phase finishes about 7 minutes after it starts.
- No extra logins: Royal soft rate-limits auth (~5 attempts / 10 min). Reuse Phase A's clients;
  order history needs the booking's `passenger_id` and `reservation_id` (already on `bookings`).
- One order-detail call per order per booking per linked account per day. The authenticated
  `CruiseClient` has no request pacing today (only the public and Princess clients do); apply the
  configured pacing to the new order calls only, so Phase B isn't slowed.
- `products_watched` is unique on `(reservation_id, product_code)`; don't reintroduce
  per-category duplicates.
- Public repo: test fixtures must be scrubbed of names, reservation/passenger ids and cabins.

## Left to the builder

- How paid prices are stored (new table vs columns), and whether "below what you paid" is a new
  alert mode or derived from stored paid prices.
- API/DTO shapes and how the Mini App code is structured.
- Child guests: compare each guest against the price for their guest type when the product has
  one; otherwise the adult price.
- Whether the alert editor lets a booked item go back to a hand-set threshold.

## Testing

- First, capture one real order-history summary and order detail from the user's account,
  scrub it, and commit it as a fixture; build the parser tests on it. Confirm the jdeath field
  mapping against it before relying on it.
- Unit tests for: per-day math, cancelled guests skipped, $0 skipped, latest order wins,
  cabin-total comparison (including the pair-deal case → no alert), savings figure, orders merged
  across two accounts, and the alert rules, especially "morning after a rebook → no alert" and
  "order fetch failed → no alert, tags kept".

## Related, not in scope

The public sweep runs *before* the personal price check and is spread over up to 150 minutes.
It tracks 0 sailings today, so it costs nothing, but if the public tier grows, personal
alerts would land well after 7am. Running it after Phase B would fix that; it's a separate change.
