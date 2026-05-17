use royaltracker_types::{AlertMode, Diff, PriceSnapshot, WatchedProduct};

/// Minimum absolute drop (USD) to consider noteworthy in `AnyDrop` mode.
pub const MIN_DROP_ABS: f64 = 1.0;
/// Minimum percentage drop to consider noteworthy in `AnyDrop` mode.
pub const MIN_DROP_PCT: f64 = 1.0;

/// Compute a diff between the latest persisted snapshot and the freshly-fetched
/// one. Honors the watch's `alert_mode`:
///
/// - `AnyDrop`: requires both `>= MIN_DROP_ABS` and `>= MIN_DROP_PCT%` to
///   suppress tiny noise.
/// - `BelowThreshold`: only fires when the new price crosses below the user's
///   threshold (was at-or-above, now strictly below). This makes it a one-shot
///   per crossing rather than firing on every drop while below.
pub fn detect_diff(
    watch: &WatchedProduct,
    previous: Option<&PriceSnapshot>,
    current: &PriceSnapshot,
) -> Option<Diff> {
    let prev = previous?;
    let old = prev.adult_promo_price?;
    let new = current.adult_promo_price?;
    if new >= old {
        return None;
    }

    match watch.alert_mode {
        AlertMode::AnyDrop => {
            let drop_abs = old - new;
            if drop_abs < MIN_DROP_ABS {
                return None;
            }
            let drop_pct = (drop_abs / old) * 100.0;
            if drop_pct < MIN_DROP_PCT {
                return None;
            }
        }
        AlertMode::BelowThreshold => {
            let threshold = watch.alert_threshold?;
            if new >= threshold {
                return None;
            }
            // Only fire on the crossing, not on every subsequent drop below.
            if old < threshold {
                return None;
            }
        }
    }
    Some(Diff::from_prices(watch.id, old, new))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn watch(mode: AlertMode, threshold: Option<f64>) -> WatchedProduct {
        WatchedProduct {
            id: 1,
            reservation_id: "R".into(),
            category_prefix: "C".into(),
            product_code: "P".into(),
            label: None,
            active: true,
            alert_mode: mode,
            alert_threshold: threshold,
        }
    }

    fn snap(price: Option<f64>) -> PriceSnapshot {
        PriceSnapshot {
            watched_id: 1,
            fetched_at: Utc::now(),
            adult_promo_price: price,
            child_promo_price: None,
            raw_response: serde_json::Value::Null,
        }
    }

    #[test]
    fn below_threshold_fires_on_crossing() {
        let w = watch(AlertMode::BelowThreshold, Some(15.99));
        let prev = snap(Some(16.50));
        let cur = snap(Some(14.99));
        assert!(detect_diff(&w, Some(&prev), &cur).is_some());
    }

    #[test]
    fn below_threshold_silent_when_already_below() {
        let w = watch(AlertMode::BelowThreshold, Some(15.99));
        let prev = snap(Some(15.50));
        let cur = snap(Some(14.99));
        assert!(detect_diff(&w, Some(&prev), &cur).is_none());
    }

    #[test]
    fn below_threshold_silent_when_still_above() {
        let w = watch(AlertMode::BelowThreshold, Some(10.00));
        let prev = snap(Some(20.00));
        let cur = snap(Some(15.00));
        assert!(detect_diff(&w, Some(&prev), &cur).is_none());
    }

    #[test]
    fn any_drop_ignores_tiny_noise() {
        let w = watch(AlertMode::AnyDrop, None);
        let prev = snap(Some(100.00));
        let cur = snap(Some(99.50)); // 50 cents, below MIN_DROP_ABS
        assert!(detect_diff(&w, Some(&prev), &cur).is_none());
    }

    #[test]
    fn any_drop_fires_on_real_drop() {
        let w = watch(AlertMode::AnyDrop, None);
        let prev = snap(Some(100.00));
        let cur = snap(Some(85.00));
        assert!(detect_diff(&w, Some(&prev), &cur).is_some());
    }
}
