use serde::Deserialize;

/// Personalized price for a single product as returned by the authenticated
/// `catalog/v2/.../products/{code}` REST endpoint.
///
/// Phase-1 ground truth: the live response shape is
///   payload.bookingOfferingData.offerings[N].pricing.{adultPromotionalPrice,
///                                                     adultShipboardPrice, currencyIso, ...}
/// (not `payload.startingFromPrice` as the original brief described). The offerings
/// array often has multiple variants; we walk it and take the first one with a
/// non-null `adultPromotionalPrice`.
#[derive(Debug, Clone)]
pub struct ProductPrice {
    pub product_code: String,
    pub adult_promo: Option<f64>,
    pub child_promo: Option<f64>,
    pub adult_shipboard: Option<f64>,
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
    currency_iso: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct Offering {
    #[serde(default)]
    pricing: Option<OfferingPricing>,
}

impl ProductPrice {
    pub fn from_raw(raw: serde_json::Value) -> Self {
        let payload = raw.get("payload").cloned().unwrap_or(serde_json::Value::Null);
        let product_code = payload
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .unwrap_or_default();

        let offerings: Vec<Offering> = payload
            .get("bookingOfferingData")
            .and_then(|b| b.get("offerings"))
            .and_then(|o| serde_json::from_value(o.clone()).ok())
            .unwrap_or_default();

        // Take the first offering with a populated adultPromotionalPrice.
        let chosen = offerings
            .into_iter()
            .find_map(|o| {
                let p = o.pricing?;
                if p.adult_promotional_price.is_some() {
                    Some(p)
                } else {
                    None
                }
            })
            .unwrap_or_default();

        Self {
            product_code,
            adult_promo: chosen.adult_promotional_price,
            child_promo: chosen.child_promotional_price,
            adult_shipboard: chosen.adult_shipboard_price,
            currency: chosen.currency_iso,
            raw,
        }
    }
}
