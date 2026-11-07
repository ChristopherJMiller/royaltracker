//! What a booking's cabin has already bought (Cruise Planner order history),
//! compared with today's price.

use crate::diff::{MIN_DROP_ABS, MIN_DROP_PCT};
use chrono::{DateTime, Utc};
use royaltracker_api::OrderLine;
use royaltracker_types::{format_money, BookedItem, Brand, GuestType, PaidUnit};
use std::collections::BTreeMap;

/// Prices within half a cent are the same price.
const CENT: f64 = 0.005;

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// One booking as one account sees it: everything needed to read that
/// account's order history for the booking. Order history is relative to the
/// account asking, so a booking shared by two accounts yields two targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderTarget {
    pub brand: Brand,
    pub reservation_id: String,
    pub ship_code: String,
    /// Sail date in the wire format `YYYYMMDD`.
    pub sail_wire: String,
    /// This account's passenger id on the booking.
    pub passenger_id: String,
}

/// Turn a booking's order lines into one [`BookedItem`] per product: what the
/// cabin paid per unit. Per guest and product only the latest order counts (a
/// rebook bought before the old order was cancelled). Guests of other
/// reservations on a shared order are ignored; per-day items need `nights`.
pub fn summarize_orders(
    reservation_id: &str,
    nights: Option<i32>,
    lines: &[OrderLine],
    now: DateTime<Utc>,
) -> Vec<BookedItem> {
    let mut latest: BTreeMap<(&str, &str), &OrderLine> = BTreeMap::new();
    for l in lines {
        if l.guest_reservation_id.as_deref().is_some_and(|r| r != reservation_id) {
            continue;
        }
        let key = (l.product_code.as_str(), l.guest_id.as_str());
        let newer = latest
            .get(&key)
            .is_none_or(|prev| (l.order_date, &l.order_code) > (prev.order_date, &prev.order_code));
        if newer {
            latest.insert(key, l);
        }
    }

    let nights = nights.filter(|n| *n > 0);
    let per_unit = |l: &OrderLine| -> Option<f64> {
        let mut p = l.subtotal;
        if l.unit != PaidUnit::Each {
            p /= f64::from(nights?);
        }
        if l.quantity > 0 {
            p /= f64::from(l.quantity);
        }
        Some(round2(p))
    };

    let mut by_product: BTreeMap<&str, Vec<&OrderLine>> = BTreeMap::new();
    for ((code, _), l) in latest {
        by_product.entry(code).or_default().push(l);
    }
    by_product
        .into_iter()
        .filter_map(|(code, guests)| {
            let paid = guests.iter().map(|l| per_unit(l)).collect::<Option<Vec<f64>>>()?;
            let first = guests[0];
            let adults = guests.iter().filter(|l| l.guest_type == GuestType::Adult).count() as u32;
            Some(BookedItem {
                reservation_id: reservation_id.to_string(),
                category_prefix: first.category_prefix.clone(),
                product_code: code.to_string(),
                title: first.title.clone(),
                unit: first.unit,
                adults,
                children: guests.len() as u32 - adults,
                paid_total: round2(paid.iter().sum()),
                paid_each: paid.iter().all(|p| (p - paid[0]).abs() < CENT).then_some(paid[0]),
                currency: first.currency.clone(),
                last_alert_total: None,
                orders_read_at: now,
            })
        })
        .collect()
}

/// Today's price for the same people, per unit: adults at the adult price,
/// children at the child price (the adult price when there is none).
pub fn today_total(item: &BookedItem, adult: Option<f64>, child: Option<f64>) -> Option<f64> {
    let adult = adult?;
    let child = child.unwrap_or(adult);
    Some(round2(f64::from(item.adults) * adult + f64::from(item.children) * child))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaidStatus {
    /// Rebooking everyone now saves `savings` in total.
    Cheaper { savings: f64 },
    Same,
    PaidLess,
}

fn unit_count(item: &BookedItem, nights: Option<i32>) -> Option<i32> {
    match item.unit {
        PaidUnit::Each => None,
        PaidUnit::Day | PaidUnit::Night => nights.filter(|n| *n > 0),
    }
}

/// Compare the cabin's paid total with today's total for the same people.
pub fn paid_status(item: &BookedItem, today_total: f64, nights: Option<i32>) -> PaidStatus {
    let diff = item.paid_total - today_total;
    if diff.abs() < CENT {
        PaidStatus::Same
    } else if diff > 0.0 {
        let times = unit_count(item, nights).map_or(1.0, f64::from);
        PaidStatus::Cheaper { savings: round2(diff * times) }
    } else {
        PaidStatus::PaidLess
    }
}

/// Alert when today's total first drops at least $1 below what was paid, then
/// again on each further drop below the last alerted total that clears the
/// usual noise floor. A rebook resets `last_alert_total` (storage).
pub fn should_alert_below_paid(item: &BookedItem, today_total: f64) -> bool {
    let below = item.paid_total - today_total;
    match item.last_alert_total {
        None => below >= MIN_DROP_ABS,
        Some(last) => {
            let drop = last - today_total;
            below > CENT && drop >= MIN_DROP_ABS && drop / last * 100.0 >= MIN_DROP_PCT
        }
    }
}

/// "$72.99/day" when everyone paid the same, else the cabin total:
/// "$109.49/day for 2".
pub fn paid_label(item: &BookedItem, currency: Option<&str>) -> String {
    let unit = match item.unit {
        PaidUnit::Day => "/day",
        PaidUnit::Night => "/night",
        PaidUnit::Each => "",
    };
    match item.paid_each {
        Some(each) => format!("{}{unit}", format_money(each, currency)),
        None => format!(
            "{}{unit} for {}",
            format_money(item.paid_total, currency),
            item.adults + item.children
        ),
    }
}

/// How the savings add up, e.g. "2 guests × 7 days × $4.00". Only when
/// everyone paid the same and there is more than one factor.
pub fn savings_breakdown(item: &BookedItem, today_total: f64, nights: Option<i32>) -> Option<String> {
    let each = item.paid_each?;
    let guests = item.adults + item.children;
    let diff = round2(each - today_total / f64::from(guests));
    let mut parts = Vec::new();
    if guests > 1 {
        parts.push(format!("{guests} guests"));
    }
    if let Some(n) = unit_count(item, nights) {
        let word = if item.unit == PaidUnit::Day { "days" } else { "nights" };
        parts.push(format!("{n} {word}"));
    }
    parts.push(format_money(diff, item.currency.as_deref()));
    (parts.len() > 1).then(|| parts.join(" × "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use royaltracker_api::OrderLine;
    use royaltracker_types::{GuestType, PaidUnit};

    const RES: &str = "1234567";

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap()
    }

    fn line(
        guest: &str,
        res: Option<&str>,
        ty: GuestType,
        unit: PaidUnit,
        subtotal: f64,
        qty: u32,
        date: &str,
    ) -> OrderLine {
        OrderLine {
            order_code: format!("O{date}"),
            order_date: chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok(),
            guest_id: guest.into(),
            guest_reservation_id: res.map(str::to_owned),
            guest_type: ty,
            category_prefix: "beverage".into(),
            product_code: "3222".into(),
            title: "Deluxe Beverage Package".into(),
            unit,
            subtotal,
            quantity: qty,
            currency: Some("USD".into()),
        }
    }

    fn adult(guest: &str, unit: PaidUnit, subtotal: f64) -> OrderLine {
        line(guest, None, GuestType::Adult, unit, subtotal, 1, "2026-09-12")
    }

    fn one(lines: &[OrderLine]) -> royaltracker_types::BookedItem {
        let mut items = summarize_orders(RES, Some(7), lines, now());
        assert_eq!(items.len(), 1, "{items:?}");
        items.remove(0)
    }

    #[test]
    fn per_day_two_guests_same_price() {
        let i = one(&[adult("g1", PaidUnit::Day, 510.93), adult("g2", PaidUnit::Day, 510.93)]);
        assert_eq!(i.paid_total, 145.98);
        assert_eq!(i.paid_each, Some(72.99));
        assert_eq!((i.adults, i.children), (2, 0));
        assert_eq!(i.unit, PaidUnit::Day);
        assert_eq!(i.product_code, "3222");
        assert_eq!(i.reservation_id, RES);
        assert_eq!(i.orders_read_at, now());
    }

    #[test]
    fn latest_order_wins_per_guest() {
        let i = one(&[
            line("g1", None, GuestType::Adult, PaidUnit::Day, 510.93, 1, "2026-09-12"),
            line("g1", None, GuestType::Adult, PaidUnit::Day, 482.93, 1, "2026-11-02"),
        ]);
        assert_eq!(i.paid_total, 68.99);
        assert_eq!(i.adults, 1);
    }

    #[test]
    fn other_reservation_guests_ignored() {
        let i = one(&[
            line("g1", Some(RES), GuestType::Adult, PaidUnit::Day, 510.93, 1, "2026-09-12"),
            line("g2", Some("7654321"), GuestType::Adult, PaidUnit::Day, 510.93, 1, "2026-09-12"),
            line("g3", None, GuestType::Adult, PaidUnit::Day, 510.93, 1, "2026-09-12"),
        ]);
        assert_eq!(i.adults, 2);
        assert_eq!(i.paid_total, 145.98);
    }

    #[test]
    fn per_day_without_nights_skipped() {
        let lines = [adult("g1", PaidUnit::Day, 510.93)];
        assert!(summarize_orders(RES, None, &lines, now()).is_empty());
        assert!(summarize_orders(RES, Some(0), &lines, now()).is_empty());
    }

    #[test]
    fn quantity_divides_each_price() {
        let i = one(&[line("g1", None, GuestType::Adult, PaidUnit::Each, 40.0, 2, "2026-09-12")]);
        assert_eq!(i.paid_total, 20.0);
        assert_eq!(i.paid_each, Some(20.0));
    }

    #[test]
    fn child_priced_at_adult_when_no_child_price() {
        let i = one(&[
            adult("g1", PaidUnit::Day, 482.93),
            adult("g2", PaidUnit::Day, 482.93),
            line("g3", None, GuestType::Child, PaidUnit::Day, 482.93, 1, "2026-09-12"),
        ]);
        assert_eq!((i.adults, i.children), (2, 1));
        assert_eq!(today_total(&i, Some(68.99), None), Some(206.97));
        assert_eq!(today_total(&i, None, Some(10.0)), None);
    }

    #[test]
    fn cheaper_same_paid_less() {
        let i = one(&[adult("g1", PaidUnit::Day, 510.93), adult("g2", PaidUnit::Day, 510.93)]);
        assert_eq!(paid_status(&i, 137.98, Some(7)), PaidStatus::Cheaper { savings: 56.0 });
        assert_eq!(paid_status(&i, 145.98, Some(7)), PaidStatus::Same);
        assert_eq!(paid_status(&i, 150.0, Some(7)), PaidStatus::PaidLess);
    }

    #[test]
    fn pair_deal_is_not_cheaper() {
        let i = one(&[adult("g1", PaidUnit::Day, 510.93), adult("g2", PaidUnit::Day, 255.50)]);
        assert_eq!(i.paid_total, 109.49);
        assert_eq!(i.paid_each, None);
        assert_eq!(paid_status(&i, 137.98, Some(7)), PaidStatus::PaidLess);
        assert!(!should_alert_below_paid(&i, 137.98));
    }

    #[test]
    fn alert_rules() {
        let mut i = one(&[adult("g1", PaidUnit::Day, 510.93), adult("g2", PaidUnit::Day, 510.93)]);
        assert!(should_alert_below_paid(&i, 137.98));
        assert!(!should_alert_below_paid(&i, 145.50)); // under $1 below what was paid
        i.last_alert_total = Some(137.98);
        assert!(!should_alert_below_paid(&i, 137.50)); // under the noise floor
        assert!(should_alert_below_paid(&i, 130.00));
    }

    #[test]
    fn labels() {
        let day = one(&[adult("g1", PaidUnit::Day, 510.93), adult("g2", PaidUnit::Day, 510.93)]);
        assert_eq!(paid_label(&day, None), "$72.99/day");
        let night = one(&[adult("g1", PaidUnit::Night, 293.93)]);
        assert_eq!(paid_label(&night, None), "$41.99/night");
        let pair = one(&[adult("g1", PaidUnit::Day, 510.93), adult("g2", PaidUnit::Day, 255.50)]);
        assert_eq!(paid_label(&pair, None), "$109.49/day for 2");
        let pass = one(&[adult("g1", PaidUnit::Each, 219.99)]);
        assert_eq!(paid_label(&pass, None), "$219.99");
        let passes = one(&[adult("g1", PaidUnit::Each, 249.99), adult("g2", PaidUnit::Each, 189.99)]);
        assert_eq!(paid_label(&passes, None), "$439.98 for 2");
    }

    #[test]
    fn breakdown() {
        let day = one(&[adult("g1", PaidUnit::Day, 510.93), adult("g2", PaidUnit::Day, 510.93)]);
        assert_eq!(
            savings_breakdown(&day, 137.98, Some(7)).as_deref(),
            Some("2 guests × 7 days × $4.00")
        );
        let night = one(&[adult("g1", PaidUnit::Night, 293.93)]);
        assert_eq!(savings_breakdown(&night, 38.99, Some(7)).as_deref(), Some("7 nights × $3.00"));
        let pass = one(&[adult("g1", PaidUnit::Each, 219.99)]);
        assert_eq!(savings_breakdown(&pass, 189.99, Some(7)), None);
        let pair = one(&[adult("g1", PaidUnit::Day, 510.93), adult("g2", PaidUnit::Day, 255.50)]);
        assert_eq!(savings_breakdown(&pair, 100.0, Some(7)), None);
    }
}
