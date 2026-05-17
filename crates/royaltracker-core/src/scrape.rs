use chrono::Utc;
use royaltracker_api::{CruiseClient, ProductPrice};
use royaltracker_storage::{CatalogEntry, PriceRepo};
use royaltracker_telegram::{send_diff, Bot, DiffContext};
use royaltracker_types::{Diff, PriceSnapshot, WatchedProduct};
use std::collections::HashMap;
use tracing::{info, warn};

use crate::diff::detect_diff;

pub struct ScrapeOutcome {
    pub snapshots_written: usize,
    pub diffs_detected: usize,
    pub diffs_notified: usize,
}

pub async fn run_scrape_cycle(
    api: &CruiseClient,
    repo: &(dyn PriceRepo),
    bot: &Bot,
    chat_id: i64,
    watched: &[WatchedProduct],
    resolve_context: impl Fn(&WatchedProduct) -> Option<ScrapeContext>,
) -> ScrapeOutcome {
    let mut snapshots_written = 0;
    let mut diffs_detected = 0;
    let mut diffs_notified = 0;
    let mut diffs_to_mark: Vec<i64> = Vec::new();

    // Pre-load the catalog cache so diff messages can include MSRP. Keyed by
    // (reservation_id, product_code). Cheap — one DB query per unique reservation.
    let mut catalog_index: HashMap<(String, String), CatalogEntry> = HashMap::new();
    let mut loaded_reservations: std::collections::HashSet<String> = Default::default();
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

    for w in watched {
        let Some(ctx) = resolve_context(w) else {
            warn!(watched_id = w.id, "no context resolved; skipping");
            continue;
        };

        let price = match api
            .fetch_product_price(
                &ctx.ship_code,
                &w.category_prefix,
                &w.product_code,
                &w.reservation_id,
                &ctx.passenger_id,
                ctx.start_date,
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

        if let Some(diff) = detect_diff(w, previous.as_ref(), &current) {
            match repo.insert_diff(&diff).await {
                Ok(diff_id) => {
                    diffs_detected += 1;
                    let label = w.label.clone().unwrap_or_else(|| w.product_code.clone());
                    let catalog_entry = catalog_index
                        .get(&(w.reservation_id.clone(), w.product_code.clone()));
                    let msrp_label = catalog_entry.and_then(|e| e.base_price_label.as_deref());
                    let context = DiffContext {
                        label: &label,
                        diff: &diff,
                        msrp_label,
                    };
                    match send_diff(bot, chat_id, &context).await {
                        Ok(()) => {
                            diffs_notified += 1;
                            diffs_to_mark.push(diff_id);
                        }
                        Err(e) => warn!(error = %e, "telegram send failed"),
                    }
                }
                Err(e) => warn!(error = %e, watched_id = w.id, "diff insert failed"),
            }
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

#[derive(Debug, Clone)]
pub struct ScrapeContext {
    pub ship_code: String,
    pub passenger_id: String,
    pub start_date: chrono::NaiveDate,
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
