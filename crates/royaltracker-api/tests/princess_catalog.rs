use royaltracker_api::princess::{apply_product_details, parse_search_hits};
use royaltracker_api::PublicProduct;

fn fixture(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/fixtures/princess/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}
fn parse(v: &serde_json::Value, cat: &str, cur: &str) -> Vec<PublicProduct> {
    parse_search_hits(v, cat, cur)
}

#[test]
fn excursion_hit_maps_price_and_unit() {
    let p = parse(&fixture("search_excursions.json"), "excursions", "USD");
    assert!(!p.is_empty());
    let x = &p[0];
    assert!(x.promo_cents.unwrap() > 0);
    assert_eq!(x.unit_label.as_deref(), Some("per adult"));
    assert_eq!(x.currency.as_deref(), Some("USD"));
    assert!(x.promo_label.as_deref().unwrap().starts_with('$'));
    assert_eq!(x.category_name, "Excursions");
    assert_eq!(x.product_code, "BZE-100");
    assert_eq!(x.promo_cents, Some(9995));
    assert_eq!(x.base_cents, None);
    assert_eq!(x.child_cents, None);
}

#[test]
fn wifi_list_price_becomes_base() {
    let p = parse(&fixture("search_wifi.json"), "wifi", "USD");
    let w = p.iter().find(|p| p.product_code == "wifi-single-device").unwrap();
    assert!(w.base_cents.unwrap() > w.promo_cents.unwrap());
    assert_eq!(w.base_cents, Some(2999));
    assert_eq!(w.category_name, "Wi-Fi");
}

#[test]
fn list_equal_to_sale_is_not_a_base() {
    let p = parse(&fixture("search_food_aud.json"), "food-and-drinks", "AUD");
    let x = p.iter().find(|p| p.product_code == "2323").unwrap();
    assert_eq!(x.base_cents, None);
}

#[test]
fn child_pricebook_maps_child_cents() {
    let v = serde_json::json!({"hits":[{"productId":"X-1","productName":"X","price":50.0,
        "c_priceRanges":[
            {"pricebook":"pcl-usd-sale-adult","minPrice":50.0,"maxPrice":50.0},
            {"pricebook":"pcl-usd-sale-child","minPrice":35.5,"maxPrice":35.5},
            {"pricebook":"pcl-usd-list-adult","minPrice":60.0,"maxPrice":60.0}]}],"total":1});
    let p = parse(&v, "excursions", "USD");
    assert_eq!(p[0].child_cents, Some(3550));
    assert_eq!(p[0].base_cents, Some(6000));
}

#[test]
fn details_fill_per_day_unit() {
    let mut p = parse(&fixture("search_wifi.json"), "wifi", "USD");
    apply_product_details(&mut p, &fixture("products_wifi.json"));
    assert_eq!(p[0].unit_label.as_deref(), Some("per person / day"));
}

#[test]
fn aud_labels_use_code() {
    let p = parse(&fixture("search_food_aud.json"), "food-and-drinks", "AUD");
    assert!(p[0].promo_label.as_deref().unwrap().starts_with("AUD "));
}

#[test]
fn empty_search_is_empty_vec() {
    assert!(parse(&fixture("search_empty.json"), "excursions", "USD").is_empty());
}
