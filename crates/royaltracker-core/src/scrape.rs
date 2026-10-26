use chrono::{NaiveDate, Utc};
use crate::princess::PrincessPriceCache;
use royaltracker_api::princess::PrincessGuestClient;
use royaltracker_api::{CruiseClient, ProductPrice};
use royaltracker_notify::{NotifyTarget, Notifier, PriceDropAlert};
use royaltracker_storage::{CatalogEntry, PriceRepo};
use royaltracker_types::{Booking, Brand, Diff, PriceSnapshot, WatchedProduct};
use std::collections::{HashMap, HashSet};
use tracing::{info, warn};

use crate::diff::detect_diff;

pub struct ScrapeOutcome {
    pub snapshots_written: usize,
    pub diffs_detected: usize,
    pub diffs_notified: usize,
    /// Watches not fetched because their booking has sailed.
    pub skipped_departed: usize,
}

/// One scrape pass over all active watched products.
///
/// Deduplicated by reservation: each watched product is fetched **once** per
/// cycle regardless of how many users subscribe to the underlying booking.
/// When a price drop is detected, the alert is fanned out to every subscriber
/// (their telegram chat id) of that reservation.
///
/// `clients_by_brand` is the pool of authenticated clients to use for price
/// fetches — typically one per brand, sourced from any user's discovery
/// login. A watched product whose booking's brand has no client in the pool
/// is skipped with a warning. Watches on bookings that have sailed (as of
/// `today`, UTC) are skipped without a fetch.
pub async fn run_scrape_cycle(
    clients_by_brand: &HashMap<Brand, CruiseClient>,
    princess: Option<&PrincessGuestClient>,
    repo: &(dyn PriceRepo),
    notifier: &dyn Notifier,
    watched: &[WatchedProduct],
    bookings_by_res: &HashMap<String, Booking>,
    today: NaiveDate,
) -> ScrapeOutcome {
    let mut snapshots_written = 0;
    let mut diffs_detected = 0;
    let mut diffs_notified = 0;
    let mut diffs_to_mark: Vec<i64> = Vec::new();
    let (watched, skipped_departed) = watches_to_price(watched, bookings_by_res, today);
    if skipped_departed > 0 {
        info!(skipped_departed, "skipping watches on bookings that have sailed");
    }

    // Pre-load the catalog cache so diff messages can include MSRP. Keyed by
    // (reservation_id, product_code). One DB query per unique reservation
    // referenced by an active watch.
    let mut catalog_index: HashMap<(String, String), CatalogEntry> = HashMap::new();
    let mut loaded_reservations: HashSet<String> = Default::default();
    for w in &watched {
        if loaded_reservations.insert(w.reservation_id.clone()) {
            if let Ok(entries) = repo.list_catalog_by_reservation(&w.reservation_id).await {
                for e in entries {
                    catalog_index
                        .insert((e.reservation_id.clone(), e.product_code.clone()), e);
                }
            }
        }
    }

    // Cache subscriber lookups per reservation so a booking with many
    // watched products only hits the join once.
    let mut subscribers_cache: HashMap<String, Vec<i64>> = HashMap::new();
    let mut princess_cache = PrincessPriceCache::default();

    for w in watched {
        let Some(booking) = bookings_by_res.get(&w.reservation_id) else {
            warn!(
                watched_id = w.id,
                reservation = %w.reservation_id,
                "no booking row; skipping"
            );
            continue;
        };

        // The unit comes from the live Princess product; RCG falls back to the
        // catalog entry's unit (below).
        let (current, live_unit) = if booking.brand == Brand::Princess {
            // Princess never uses the RCG client pool.
            let Some(pc) = princess else {
                warn!(watched_id = w.id, "no Princess client; skipping");
                continue;
            };
            let Some(voyage) = booking.voyage_code.as_deref() else {
                warn!(watched_id = w.id, "Princess booking has no voyage_code; skipping");
                continue;
            };
            let currency = booking.currency.as_deref().unwrap_or("USD");
            let products = match princess_cache.products(pc, voyage, currency).await {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = %e, watched_id = w.id, "princess product fetch failed");
                    continue;
                }
            };
            let Some(product) = products.get(&w.product_code) else {
                warn!(watched_id = w.id, product = %w.product_code, "product not in Princess catalog; skipping");
                continue;
            };
            let snap = PriceSnapshot {
                watched_id: w.id,
                fetched_at: Utc::now(),
                adult_promo_price: product.promo_dollars(),
                child_promo_price: product.child_cents.map(|c| c as f64 / 100.0),
                raw_response: serde_json::to_value(product).unwrap_or(serde_json::Value::Null),
            };
            (snap, product.unit_label.clone())
        } else {
            let Some(api) = clients_by_brand.get(&booking.brand) else {
                warn!(
                    watched_id = w.id,
                    brand = %booking.brand,
                    "no authenticated client for brand; skipping"
                );
                continue;
            };

            let Some(passenger_id) = booking.passenger_id.as_deref() else {
                warn!(watched_id = w.id, "booking has no passenger_id; skipping");
                continue;
            };

            let price = match api
                .fetch_product_price(
                    &booking.ship_code,
                    &w.category_prefix,
                    &w.product_code,
                    &w.reservation_id,
                    passenger_id,
                    booking.sail_date,
                )
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = %e, watched_id = w.id, "product fetch failed");
                    continue;
                }
            };
            (into_snapshot(w.id, &price), None)
        };

        let previous = repo.latest_snapshot(w.id).await.ok().flatten();
        if let Err(e) = repo.insert_snapshot(&current).await {
            warn!(error = %e, watched_id = w.id, "snapshot insert failed");
            continue;
        }
        snapshots_written += 1;

        let Some(diff) = detect_diff(w, previous.as_ref(), &current) else {
            continue;
        };

        let diff_id = match repo.insert_diff(&diff).await {
            Ok(id) => id,
            Err(e) => {
                warn!(error = %e, watched_id = w.id, "diff insert failed");
                continue;
            }
        };
        diffs_detected += 1;

        // Resolve fan-out targets (cached per reservation).
        let chat_ids = match subscribers_cache.get(&w.reservation_id) {
            Some(v) => v.clone(),
            None => {
                let subs = repo
                    .list_subscribers_for_reservation(&w.reservation_id)
                    .await
                    .unwrap_or_default();
                let ids: Vec<i64> = subs.into_iter().map(|s| s.telegram_chat_id).collect();
                subscribers_cache.insert(w.reservation_id.clone(), ids.clone());
                ids
            }
        };

        if chat_ids.is_empty() {
            warn!(
                watched_id = w.id,
                reservation = %w.reservation_id,
                "diff detected but no active subscribers to notify"
            );
            // Still mark notified so we don't re-process this diff forever.
            diffs_to_mark.push(diff_id);
            continue;
        }

        let label = w.label.clone().unwrap_or_else(|| w.product_code.clone());
        let catalog_entry = catalog_index
            .get(&(w.reservation_id.clone(), w.product_code.clone()));
        let msrp_label = catalog_entry.and_then(|e| e.base_price_label.as_deref());
        let unit_label = live_unit
            .as_deref()
            .or_else(|| catalog_entry.and_then(|e| e.unit_label.as_deref()));
        let itinerary = itinerary_label(booking);
        let alert = PriceDropAlert {
            label: &label,
            diff: &diff,
            msrp_label,
            itinerary: Some(&itinerary),
            manage_url: None,
            currency: booking.currency.as_deref(),
            unit_label,
        };

        let mut any_sent = false;
        for chat_id in chat_ids {
            let target = NotifyTarget::Telegram { chat_id };
            match notifier.notify_price_drop(&target, &alert).await {
                Ok(()) => {
                    diffs_notified += 1;
                    any_sent = true;
                }
                Err(e) => warn!(error = %e, chat_id, "notify send failed"),
            }
        }
        if any_sent {
            diffs_to_mark.push(diff_id);
        }
    }

    if !diffs_to_mark.is_empty() {
        if let Err(e) = repo.mark_notified(&diffs_to_mark).await {
            warn!(error = %e, "mark_notified failed");
        }
    }

    info!(
        snapshots_written,
        diffs_detected, diffs_notified, "scrape cycle complete"
    );
    ScrapeOutcome {
        snapshots_written,
        diffs_detected,
        diffs_notified,
        skipped_departed,
    }
}

/// The watches still worth pricing, plus how many were dropped because their
/// booking has sailed. A watch with no booking row is kept so the loop logs it.
fn watches_to_price<'a>(
    watched: &'a [WatchedProduct],
    bookings_by_res: &HashMap<String, Booking>,
    today: NaiveDate,
) -> (Vec<&'a WatchedProduct>, usize) {
    let (live, departed): (Vec<_>, Vec<_>) = watched.iter().partition(|w| {
        bookings_by_res
            .get(&w.reservation_id)
            .is_none_or(|b| !b.has_departed(today))
    });
    (live, departed.len())
}

/// Human label for the sailing a price alert belongs to, e.g.
/// "Wonder of the Seas · 3 nights · Sep 4, 2026 · cabin 9434". Prefers the
/// recovered GTY cabin when we have one, otherwise the displayed stateroom
/// (rendered as "GTY" while unassigned).
fn itinerary_label(b: &Booking) -> String {
    let ship = royaltracker_types::ship_name(b.brand, &b.ship_code).unwrap_or(b.ship_code.as_str());
    let mut s = ship.to_string();
    if let Some(n) = b.nights {
        s.push_str(&format!(" · {n} night{}", if n == 1 { "" } else { "s" }));
    }
    s.push_str(&format!(" · {}", b.sail_date.format("%b %-d, %Y")));
    // Prefer the recovered GTY cabin, else the displayed room (skip bare "GTY").
    let effective_cabin = b
        .assigned_stateroom
        .as_deref()
        .or_else(|| b.stateroom.as_deref().filter(|r| *r != "GTY"));
    if let Some(cabin) = effective_cabin {
        let suffix = if b.assigned_stateroom.is_some() {
            " (assigned)"
        } else {
            ""
        };
        s.push_str(&format!(" · cabin {cabin}{suffix}"));
        if let Some(deck) = royaltracker_types::deck_of_cabin(cabin) {
            s.push_str(&format!(" · deck {deck}"));
        } else if let Some(name) = &b.deck_name {
            s.push_str(&format!(" · {name} deck"));
        }
    } else if b.stateroom.as_deref() == Some("GTY") {
        s.push_str(" · GTY");
    }
    s
}

fn into_snapshot(watched_id: i64, p: &ProductPrice) -> PriceSnapshot {
    PriceSnapshot {
        watched_id,
        fetched_at: Utc::now(),
        adult_promo_price: p.adult_promo,
        child_promo_price: p.child_promo,
        raw_response: p.raw.clone(),
    }
}

// Silence the unused-import warning if Diff is not used directly.
#[allow(dead_code)]
fn _diff_compile_check(d: Diff) -> Diff {
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use royaltracker_types::AlertMode;

    fn watch(id: i64, res: &str) -> WatchedProduct {
        WatchedProduct {
            id,
            reservation_id: res.into(),
            category_prefix: "pkg".into(),
            product_code: format!("P{id}"),
            label: None,
            active: true,
            alert_mode: AlertMode::AnyDrop,
            alert_threshold: None,
        }
    }

    fn booking(res: &str, sail: NaiveDate) -> Booking {
        Booking {
            reservation_id: res.into(),
            brand: Brand::Royal,
            account_id: "a".into(),
            ship_code: "ST".into(),
            sail_date: sail,
            passenger_id: None,
            nights: Some(7),
            package_code: None,
            stateroom: None,
            assigned_stateroom: None,
            voyage_code: None,
            currency: None,
            deck_name: None,
            cabin_category: None,
        }
    }

    #[test]
    fn sailed_bookings_are_not_priced() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let bookings: HashMap<String, Booking> = [
            booking("1111111", NaiveDate::from_ymd_opt(2026, 9, 4).unwrap()),
            booking("2222222", today),
            booking("3333333", NaiveDate::from_ymd_opt(2027, 2, 7).unwrap()),
        ]
        .into_iter()
        .map(|b| (b.reservation_id.clone(), b))
        .collect();
        let watched = vec![
            watch(1, "1111111"),
            watch(2, "2222222"),
            watch(3, "3333333"),
            watch(4, "3333333"),
            watch(5, "9999999"),
        ];
        let (live, skipped) = watches_to_price(&watched, &bookings, today);
        assert_eq!(skipped, 2);
        // Upcoming watches stay, and an orphan watch is kept so the loop logs it.
        assert_eq!(live.iter().map(|w| w.id).collect::<Vec<_>>(), vec![3, 4, 5]);
    }

    #[test]
    fn alert_label_names_a_princess_deck() {
        let mut b = booking("AB12CD", NaiveDate::from_ymd_opt(2027, 3, 1).unwrap());
        b.brand = Brand::Princess;
        b.ship_code = "XP".into();
        b.stateroom = Some("D123".into());
        b.deck_name = Some("Dolphin".into());
        assert_eq!(
            itinerary_label(&b),
            "Discovery Princess · 7 nights · Mar 1, 2027 · cabin D123 · Dolphin deck"
        );
        b.brand = Brand::Royal;
        b.ship_code = "ST".into();
        b.stateroom = Some("9434".into());
        assert!(itinerary_label(&b).ends_with("· cabin 9434 · deck 9"), "a numbered deck wins");
    }
}
