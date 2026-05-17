use chrono::Utc;
use royaltracker_api::{CruiseClient, ProductPrice};
use royaltracker_storage::{CatalogEntry, PriceRepo};
use royaltracker_telegram::{send_diff, Bot, DiffContext};
use royaltracker_types::{Booking, Brand, Diff, PriceSnapshot, WatchedProduct};
use std::collections::{HashMap, HashSet};
use tracing::{info, warn};

use crate::diff::detect_diff;

pub struct ScrapeOutcome {
    pub snapshots_written: usize,
    pub diffs_detected: usize,
    pub diffs_notified: usize,
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
/// is skipped with a warning.
pub async fn run_scrape_cycle(
    clients_by_brand: &HashMap<Brand, CruiseClient>,
    repo: &(dyn PriceRepo),
    bot: &Bot,
    watched: &[WatchedProduct],
    bookings_by_res: &HashMap<String, Booking>,
) -> ScrapeOutcome {
    let mut snapshots_written = 0;
    let mut diffs_detected = 0;
    let mut diffs_notified = 0;
    let mut diffs_to_mark: Vec<i64> = Vec::new();

    // Pre-load the catalog cache so diff messages can include MSRP. Keyed by
    // (reservation_id, product_code). One DB query per unique reservation
    // referenced by an active watch.
    let mut catalog_index: HashMap<(String, String), CatalogEntry> = HashMap::new();
    let mut loaded_reservations: HashSet<String> = Default::default();
    for w in watched {
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

    for w in watched {
        let Some(booking) = bookings_by_res.get(&w.reservation_id) else {
            warn!(
                watched_id = w.id,
                reservation = %w.reservation_id,
                "no booking row; skipping"
            );
            continue;
        };

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

        let current = into_snapshot(w.id, &price);

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
        let context = DiffContext {
            label: &label,
            diff: &diff,
            msrp_label,
        };

        let mut any_sent = false;
        for chat_id in chat_ids {
            match send_diff(bot, chat_id, &context).await {
                Ok(()) => {
                    diffs_notified += 1;
                    any_sent = true;
                }
                Err(e) => warn!(error = %e, chat_id, "telegram send failed"),
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
    }
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
