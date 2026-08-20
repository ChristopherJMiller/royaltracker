//! The paced public-promo sweep. Fetches each distinct sailing's public prices
//! ONCE (appkey-only), records a snapshot per tracked product, detects drops,
//! and fans out to public subscribers. Fetch failures are recorded (fixing the
//! silent-freeze bug) rather than swallowed.

use chrono::{NaiveDate, Utc};
use royaltracker_api::{PublicClient, PublicFetch, PublicProduct};
use royaltracker_notify::{NotifyTarget, Notifier, PriceDropAlert, PushSubscription};
use royaltracker_storage::{PriceRepo, SailingDiff, SailingSnapshot, ScrapeTarget};
use royaltracker_types::{AlertMode, Brand, Diff, PublicChannelKind};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{info, warn};

use crate::diff::{MIN_DROP_ABS, MIN_DROP_PCT};

pub struct PublicSweepConfig {
    /// Total wall-clock window to spread the sweep across (drip).
    pub window: Duration,
}

impl Default for PublicSweepConfig {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(150 * 60),
        }
    }
}

#[derive(Debug, Default)]
pub struct PublicSweepOutcome {
    pub sailings_fetched: usize,
    pub prices_recorded: usize,
    pub diffs_detected: usize,
    pub diffs_notified: usize,
    pub failures: usize,
}

/// Does this drop qualify to alert a subscriber with the given mode/threshold?
/// Mirrors `detect_diff` so public and authed tiers behave identically.
fn qualifies(mode: AlertMode, threshold: Option<f64>, old: f64, new: f64) -> bool {
    if new >= old {
        return false;
    }
    match mode {
        AlertMode::AnyDrop => {
            let drop_abs = old - new;
            drop_abs >= MIN_DROP_ABS && (drop_abs / old) * 100.0 >= MIN_DROP_PCT
        }
        AlertMode::BelowThreshold => match threshold {
            Some(t) => new < t && old >= t, // fire only on the crossing
            None => false,
        },
    }
}

fn map_target(kind: PublicChannelKind, endpoint: &str, p256dh: Option<&str>, auth: Option<&str>) -> NotifyTarget {
    match kind {
        PublicChannelKind::WebPush => NotifyTarget::WebPush(Box::new(PushSubscription {
            endpoint: endpoint.to_string(),
            p256dh: p256dh.unwrap_or_default().to_string(),
            auth: auth.unwrap_or_default().to_string(),
        })),
        PublicChannelKind::Email => NotifyTarget::Email {
            address: endpoint.to_string(),
        },
    }
}

fn sailing_label(ship_code: &str, sail_date: NaiveDate) -> String {
    let ship = royaltracker_types::ship_name(ship_code).unwrap_or(ship_code);
    format!("{ship} · {}", sail_date.format("%b %-d, %Y"))
}

pub async fn run_public_sweep(
    client: &PublicClient,
    repo: &dyn PriceRepo,
    notifier: &dyn Notifier,
    cfg: &PublicSweepConfig,
) -> PublicSweepOutcome {
    let mut outcome = PublicSweepOutcome::default();

    let targets = match repo.list_sailings_to_scrape().await {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "list_sailings_to_scrape failed");
            return outcome;
        }
    };

    // One fetch per distinct sailing serves every tracked product on it.
    let mut groups: HashMap<(Brand, String, NaiveDate), Vec<ScrapeTarget>> = HashMap::new();
    for t in targets {
        groups
            .entry((t.brand, t.ship_code.clone(), t.sail_date))
            .or_default()
            .push(t);
    }

    let n = groups.len();
    if n == 0 {
        info!("public sweep: no sailings to scrape");
        return outcome;
    }
    // Drip across the window, capped so a small system doesn't idle for hours
    // (the per-request limiter inside PublicClient is the primary pacing).
    let drip = (cfg.window / n as u32).min(Duration::from_secs(300));
    info!(sailings = n, drip_s = drip.as_secs(), "public sweep starting");

    for (idx, ((brand, ship, sail), tps)) in groups.into_iter().enumerate() {
        match client.fetch_public_products(brand, &ship, sail).await {
            Ok(PublicFetch::Products(products)) => {
                outcome.sailings_fetched += 1;
                let by_code: HashMap<&str, &PublicProduct> =
                    products.iter().map(|p| (p.product_code.as_str(), p)).collect();
                for tp in &tps {
                    match by_code.get(tp.product_code.as_str()) {
                        Some(p) => {
                            sweep_one(repo, notifier, &ship, sail, tp, p, &mut outcome).await
                        }
                        None => {
                            // Tracked product no longer in the catalog this pass.
                            let _ = repo
                                .note_fetch_failure(tp.tracked_id, "product absent from catalog")
                                .await;
                            outcome.failures += 1;
                        }
                    }
                }
            }
            Ok(PublicFetch::PlannerNotOpen) => {
                info!(ship = %ship, %sail, "planner not open; nothing to record");
            }
            Err(e) => {
                warn!(ship = %ship, %sail, error = %e, "public fetch failed");
                for tp in &tps {
                    let _ = repo.note_fetch_failure(tp.tracked_id, &e.to_string()).await;
                    outcome.failures += 1;
                }
            }
        }

        if idx + 1 < n && !drip.is_zero() {
            tokio::time::sleep(drip).await;
        }
    }

    info!(
        sailings_fetched = outcome.sailings_fetched,
        prices_recorded = outcome.prices_recorded,
        diffs_detected = outcome.diffs_detected,
        diffs_notified = outcome.diffs_notified,
        failures = outcome.failures,
        "public sweep complete"
    );
    outcome
}

#[allow(clippy::too_many_arguments)]
async fn sweep_one(
    repo: &dyn PriceRepo,
    notifier: &dyn Notifier,
    ship: &str,
    sail: NaiveDate,
    tp: &ScrapeTarget,
    p: &PublicProduct,
    outcome: &mut PublicSweepOutcome,
) {
    let promo = p.promo_dollars();
    let promo_present = promo.is_some();
    let prev = repo.latest_sailing_snapshot(tp.tracked_id).await.ok().flatten();

    let snap = SailingSnapshot {
        tracked_id: tp.tracked_id,
        fetched_at: Utc::now(),
        adult_promo_price: promo,
        child_promo_price: None,
        base_price: p.base_dollars(),
        promo_present,
        raw_response: serde_json::to_value(p).unwrap_or(serde_json::Value::Null),
    };
    if let Err(e) = repo.record_public_price(tp.tracked_id, &snap).await {
        warn!(tracked_id = tp.tracked_id, error = %e, "record_public_price failed");
        return;
    }
    outcome.prices_recorded += 1;

    // Only diff when a promo was present on BOTH sides — a null promo means the
    // sale ended, not a $0 drop.
    let (Some(prev_snap), true) = (&prev, promo_present) else {
        return;
    };
    let (Some(old), Some(new)) = (prev_snap.adult_promo_price, promo) else {
        return;
    };
    if new >= old {
        return;
    }

    let delta_pct = ((new - old) / old) * 100.0;
    let sd = SailingDiff {
        id: 0,
        tracked_id: tp.tracked_id,
        detected_at: Utc::now(),
        old_price: old,
        new_price: new,
        delta_pct,
        notified: false,
    };
    let diff_id = repo.insert_sailing_diff(&sd).await.ok();
    outcome.diffs_detected += 1;

    let subs = repo
        .list_public_subscriptions_for(tp.tracked_id)
        .await
        .unwrap_or_default();
    // Synthetic Diff for the alert renderer (send_diff only reads old/new/delta;
    // the tracked_id occupies the watched_id slot).
    let syn = Diff::from_prices(tp.tracked_id, old, new);
    let label = p
        .title
        .clone()
        .unwrap_or_else(|| tp.product_code.clone());
    let itinerary = sailing_label(ship, sail);

    let mut any_sent = false;
    for sub in &subs {
        if !qualifies(sub.alert_mode, sub.alert_threshold, old, new) {
            continue;
        }
        let target = map_target(sub.kind, &sub.endpoint, sub.p256dh.as_deref(), sub.auth.as_deref());
        let alert = PriceDropAlert {
            label: &label,
            diff: &syn,
            msrp_label: p.base_label.as_deref(),
            itinerary: Some(&itinerary),
            manage_url: None,
        };
        match notifier.notify_price_drop(&target, &alert).await {
            Ok(()) => {
                outcome.diffs_notified += 1;
                any_sent = true;
            }
            Err(e) => {
                // Web-push/email delivery + dead-endpoint cleanup arrive with the
                // identity phase; for now just log.
                warn!(subscription_id = sub.subscription_id, error = %e, "public notify failed");
            }
        }
    }

    // Mark the diff resolved once anyone was reached (or there was no one to
    // reach) so it isn't reprocessed forever.
    if let Some(id) = diff_id {
        if any_sent || subs.is_empty() {
            let _ = repo.mark_sailing_notified(&[id]).await;
        }
    }
}
