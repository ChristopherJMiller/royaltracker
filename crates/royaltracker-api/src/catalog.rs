use serde::Deserialize;

/// Personalized price for a single product as returned by the authenticated
/// `catalog/v2/.../products/{code}` REST endpoint.
///
/// Phase-1 ground truth: the live response shape is
///   payload.bookingOfferingData.offerings[N].pricing.{adultPromotionalPrice,
///                                                     adultShipboardPrice, currencyIso, ...}
/// (not `payload.startingFromPrice` as the original brief described). The offerings
/// array often has multiple variants; we walk it and take the first one with a
/// non-null `adultPromotionalPrice`. With no sale running there is no promo
/// price at all and the product sells at its shipboard (list) price, which
/// [`ProductPrice::adult_price`] falls back to.
#[derive(Debug, Clone)]
pub struct ProductPrice {
    pub product_code: String,
    pub adult_promo: Option<f64>,
    pub child_promo: Option<f64>,
    pub adult_shipboard: Option<f64>,
    pub child_shipboard: Option<f64>,
    pub currency: Option<String>,
    /// Whole `payload` JSON; kept for forensic / schema-drift inspection.
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OfferingPricing {
    #[serde(default)]
    adult_promotional_price: Option<f64>,
    #[serde(default)]
    child_promotional_price: Option<f64>,
    #[serde(default)]
    adult_shipboard_price: Option<f64>,
    #[serde(default)]
    child_shipboard_price: Option<f64>,
    #[serde(default)]
    currency_iso: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct Offering {
    /// e.g. `33S2-XX20990101-A`: the variant code, then the sailing.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    pricing: Option<OfferingPricing>,
}

impl ProductPrice {
    pub fn from_raw(raw: serde_json::Value) -> Self {
        Self::pick(raw, None)
    }

    /// Like [`Self::from_raw`], but prefer the offerings of `requested_code`
    /// (the variant that was booked) when the response lists several.
    pub fn from_raw_for(raw: serde_json::Value, requested_code: &str) -> Self {
        Self::pick(raw, Some(requested_code))
    }

    fn pick(raw: serde_json::Value, requested_code: Option<&str>) -> Self {
        let payload = raw.get("payload").cloned().unwrap_or(serde_json::Value::Null);
        let product_code = payload
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .unwrap_or_default();

        let mut offerings: Vec<Offering> = payload
            .get("bookingOfferingData")
            .and_then(|b| b.get("offerings"))
            .and_then(|o| serde_json::from_value(o.clone()).ok())
            .unwrap_or_default();
        // Offering ids are the variant code itself (packages: `33S1`..`33S4`)
        // or the code then the sailing (`ZHF3-XX20990101-A`). Only the
        // requested variant's offerings count; none of them means no price,
        // never another variant's.
        if let Some(code) = requested_code {
            let prefix = format!("{code}-");
            offerings.retain(|o| {
                o.id.as_deref().is_some_and(|id| id == code || id.starts_with(&prefix))
            });
        }

        // Take the first offering with a populated adultPromotionalPrice; with
        // no sale on any of them, the first priced one (list price only).
        let priced: Vec<OfferingPricing> =
            offerings.into_iter().filter_map(|o| o.pricing).collect();
        let chosen = priced
            .iter()
            .find(|p| p.adult_promotional_price.is_some())
            .or(priced.first())
            .cloned()
            .unwrap_or_default();

        Self {
            product_code,
            adult_promo: chosen.adult_promotional_price,
            child_promo: chosen.child_promotional_price,
            adult_shipboard: chosen.adult_shipboard_price,
            child_shipboard: chosen.child_shipboard_price,
            currency: chosen.currency_iso,
            raw,
        }
    }

    /// What an adult pays today: the sale price, else the list price.
    pub fn adult_price(&self) -> Option<f64> {
        self.adult_promo.or(self.adult_shipboard)
    }

    /// What a child pays today: the sale price, else the list price.
    pub fn child_price(&self) -> Option<f64> {
        self.child_promo.or(self.child_shipboard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(pricing: serde_json::Value) -> serde_json::Value {
        json!({ "payload": { "id": "ZHF3", "bookingOfferingData": {
            "offerings": [ { "id": "ZHF3-XX20990101-A", "pricing": pricing } ]
        } } })
    }

    #[test]
    fn on_sale_uses_promotional_price() {
        let p = ProductPrice::from_raw(raw(json!({
            "currencyIso": "USD",
            "adultPromotionalPrice": 257.99, "childPromotionalPrice": 257.99,
            "adultShipboardPrice": 369.0, "childShipboardPrice": 369.0,
        })));
        assert_eq!(p.adult_price(), Some(257.99));
        assert_eq!(p.child_price(), Some(257.99));
        assert_eq!(p.adult_shipboard, Some(369.0));
    }

    /// Live shape for Breezy Bay on 2026-11-06: in stock, no sale running, so
    /// it sells at full list price. That's still the price to track.
    #[test]
    fn no_sale_falls_back_to_list_price() {
        let p = ProductPrice::from_raw(raw(json!({
            "currencyIso": "USD",
            "adultPromotionalPrice": null, "childPromotionalPrice": null,
            "adultShipboardPrice": 369.0, "childShipboardPrice": 369.0,
            "adultDiscountPercentage": 0,
        })));
        assert_eq!(p.adult_price(), Some(369.0));
        assert_eq!(p.child_price(), Some(369.0));
        assert_eq!(p.currency.as_deref(), Some("USD"));
    }

    #[test]
    fn no_pricing_at_all_is_none() {
        let p = ProductPrice::from_raw(json!({ "payload": { "id": "X" } }));
        assert_eq!(p.adult_price(), None);
        assert_eq!(p.child_price(), None);
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;
    use serde_json::json;

    fn raw() -> serde_json::Value {
        json!({ "payload": { "id": "AAA1", "bookingOfferingData": { "offerings": [
            { "id": "AAA1-XX20990101-A", "pricing": { "adultPromotionalPrice": 10.0 } },
            { "id": "AAA2-XX20990101-A", "pricing": { "adultPromotionalPrice": 20.0 } }
        ] } } })
    }

    #[test]
    fn offering_matches_requested_variant() {
        assert_eq!(ProductPrice::from_raw_for(raw(), "AAA2").adult_price(), Some(20.0));
    }

    #[test]
    fn exact_offering_id_matches() {
        // Packages: offering ids are the bare variant codes (33S1..33S4).
        let raw = json!({ "payload": { "id": "33S1", "bookingOfferingData": { "offerings": [
            { "id": "33S1", "pricing": { "adultPromotionalPrice": 22.99 } },
            { "id": "33S2", "pricing": { "adultPromotionalPrice": 37.99 } }
        ] } } });
        assert_eq!(ProductPrice::from_raw_for(raw, "33S2").adult_price(), Some(37.99));
    }

    #[test]
    fn unmatched_variant_has_no_price() {
        // Never fall back to another variant's (cheaper) price.
        let p = ProductPrice::from_raw_for(raw(), "ZZZZ");
        assert_eq!(p.adult_price(), None);
        assert_eq!(p.child_price(), None);
    }

    #[test]
    fn requested_variant_without_pricing_has_no_price() {
        let raw = json!({ "payload": { "id": "33S1", "bookingOfferingData": { "offerings": [
            { "id": "33S1", "pricing": { "adultPromotionalPrice": 22.99 } },
            { "id": "33S2", "pricing": null }
        ] } } });
        assert_eq!(ProductPrice::from_raw_for(raw, "33S2").adult_price(), None);
    }
}
