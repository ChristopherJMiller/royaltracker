//! Princess public schedule (`resdb/p1.0/products`, no account). One call
//! returns every voyage on every ship for ~2 years. See spec §9.3.

use chrono::NaiveDate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincessVoyage {
    pub voyage_code: String,
    pub ship_code: String,
    pub sail_date: NaiveDate,
    pub nights: i32,
    pub itinerary: String,
    pub currency: String,
}

/// Flatten `products[].cruises[]` into voyages, sorted by
/// `(ship_code, sail_date, voyage_code)`. Entries missing a voyage code, ship
/// or parseable sail date are skipped.
pub fn parse_schedule(v: &serde_json::Value) -> Vec<PrincessVoyage> {
    let mut out = Vec::new();
    let products = v.get("products").and_then(|p| p.as_array()).into_iter().flatten();
    for p in products {
        let itinerary = p.get("name").and_then(|n| n.as_str()).unwrap_or("").trim();
        for c in p.get("cruises").and_then(|c| c.as_array()).into_iter().flatten() {
            let voyage = c.get("voyage");
            let code = c.get("id").and_then(|s| s.as_str()).unwrap_or("");
            let ship = voyage
                .and_then(|v| v.get("ship"))
                .and_then(|s| s.get("id"))
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let date = voyage
                .and_then(|v| v.get("sailDate"))
                .and_then(|s| s.as_str())
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y%m%d").ok());
            let (Some(date), false, false) = (date, code.is_empty(), ship.is_empty()) else {
                continue;
            };
            let nights = voyage
                .and_then(|v| v.get("duration"))
                .and_then(|d| d.as_i64())
                .unwrap_or(0) as i32;
            let currency = match c.get("shipCurrency").and_then(|s| s.as_str()) {
                Some(s) if !s.is_empty() => s,
                _ => "USD",
            };
            out.push(PrincessVoyage {
                voyage_code: code.to_string(),
                ship_code: ship.to_string(),
                sail_date: date,
                nights,
                itinerary: itinerary.to_string(),
                currency: currency.to_string(),
            });
        }
    }
    out.sort_by(|a, b| {
        (&a.ship_code, a.sail_date, &a.voyage_code).cmp(&(&b.ship_code, b.sail_date, &b.voyage_code))
    });
    out
}
