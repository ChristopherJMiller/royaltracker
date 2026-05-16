use royaltracker_types::{Diff, PriceSnapshot};

/// Minimum absolute drop (USD) to consider noteworthy.
pub const MIN_DROP_ABS: f64 = 1.0;
/// Minimum percentage drop to consider noteworthy.
pub const MIN_DROP_PCT: f64 = 1.0;

/// Compute a diff between the latest persisted snapshot and the freshly-fetched one.
/// Returns None unless we see a meaningful PRICE DROP — defined as both
/// `>= $MIN_DROP_ABS` AND `>= MIN_DROP_PCT%`. Increases and tiny fluctuations
/// are suppressed to avoid notification spam.
pub fn detect_diff(
    watched_id: i64,
    previous: Option<&PriceSnapshot>,
    current: &PriceSnapshot,
) -> Option<Diff> {
    let prev = previous?;
    let old = prev.adult_promo_price?;
    let new = current.adult_promo_price?;
    if new >= old {
        return None;
    }
    let drop_abs = old - new;
    if drop_abs < MIN_DROP_ABS {
        return None;
    }
    let drop_pct = (drop_abs / old) * 100.0;
    if drop_pct < MIN_DROP_PCT {
        return None;
    }
    Some(Diff::from_prices(watched_id, old, new))
}
