//! Pure parsers for Princess SFCC product-search / product-detail JSON.

use royaltracker_types::format_money;
use serde_json::Value;

use crate::public::PublicProduct;

pub fn category_name(category_id: &str) -> &'static str {
    match category_id {
        "excursions" => "Excursions",
        "food-and-drinks" => "Food & Drinks",
        "wifi" => "Wi-Fi",
        "extras" => "Extras",
        _ => "Other",
    }
}

fn to_cents(x: f64) -> i64 {
    (x * 100.0).round() as i64
}

/// Map a product-search response to products. `price` is the sellable guest
/// price; a `-list` pricebook `maxPrice` above it becomes the base price and a
/// `-child` pricebook becomes the child price.
pub fn parse_search_hits(v: &Value, category_id: &str, currency: &str) -> Vec<PublicProduct> {
    let Some(hits) = v.get("hits").and_then(|h| h.as_array()) else {
        return Vec::new();
    };
    let cur = Some(currency);
    hits.iter()
        .filter_map(|h| {
            let id = h.get("productId")?.as_str()?.to_string();
            let price = h.get("price").and_then(|p| p.as_f64());
            let promo_cents = price.map(to_cents);
            let ranges = h
                .get("c_priceRanges")
                .and_then(|r| r.as_array())
                .map(|a| a.as_slice())
                .unwrap_or(&[]);
            let range_max = |needle: &str| {
                ranges
                    .iter()
                    .filter(|r| {
                        r.get("pricebook")
                            .and_then(|p| p.as_str())
                            .is_some_and(|p| p.contains(needle))
                    })
                    .filter_map(|r| r.get("maxPrice").and_then(|m| m.as_f64()))
                    .map(to_cents)
                    .reduce(i64::max)
            };
            let base_cents = range_max("-list").filter(|b| promo_cents.is_some_and(|p| *b > p));
            let child_cents = range_max("-child");
            let label = |c: Option<i64>| c.map(|c| format_money(c as f64 / 100.0, cur));
            Some(PublicProduct {
                category_id: category_id.to_string(),
                category_name: category_name(category_id).to_string(),
                product_code: id,
                title: h
                    .get("productName")
                    .and_then(|n| n.as_str())
                    .map(str::to_string),
                promo_cents,
                base_cents,
                promo_label: label(promo_cents),
                base_label: label(base_cents),
                currency: Some(currency.to_string()),
                unit_label: (category_id == "excursions").then(|| "per adult".to_string()),
                child_cents,
            })
        })
        .collect()
}

/// Fill `unit_label` from a shopper-products detail response (`data[].unit`,
/// e.g. "person / day" becomes "per person / day").
pub fn apply_product_details(products: &mut [PublicProduct], v: &Value) {
    let Some(data) = v.get("data").and_then(|d| d.as_array()) else {
        return;
    };
    for d in data {
        let (Some(id), Some(unit)) = (
            d.get("id").and_then(|i| i.as_str()),
            d.get("unit").and_then(|u| u.as_str()),
        ) else {
            continue;
        };
        if unit.trim().is_empty() {
            continue;
        }
        for p in products.iter_mut().filter(|p| p.product_code == id) {
            p.unit_label = Some(format!("per {unit}"));
        }
    }
}
