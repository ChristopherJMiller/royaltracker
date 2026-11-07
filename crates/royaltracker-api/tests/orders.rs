//! Order-history parsing against scrubbed real responses
//! (`tests/fixtures/rcg/`, captured 2026-11-06; see the plan's Recon findings).

use chrono::NaiveDate;
use royaltracker_api::{parse_order_detail, parse_order_summary, OrderRef};
use royaltracker_types::{GuestType, PaidUnit};
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/rcg/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn order(code: &str, date: &str) -> OrderRef {
    OrderRef {
        code: code.into(),
        date: NaiveDate::parse_from_str(date, "%Y-%m-%d").ok(),
    }
}

#[test]
fn summary_skips_cancelled_orders() {
    let refs = parse_order_summary(&fixture("order_history.json")).unwrap();
    let codes: Vec<&str> = refs.iter().map(|r| r.code.as_str()).collect();
    assert_eq!(codes, ["ID-5", "ID-6", "ID-7", "ID-8"]);
    assert_eq!(refs[0].date, NaiveDate::from_ymd_opt(2026, 11, 5));
}

#[test]
fn summary_includes_others_skips_zero_and_dedupes() {
    let v = json!({"payload": {
        "myOrders": [
            {"orderCode": "A", "orderDate": "2026-11-01", "status": "BOOKED", "orderTotals": {"total": 10.0}},
            {"orderCode": "Z", "orderDate": "2026-11-01", "status": "BOOKED", "orderTotals": {"total": 0}}
        ],
        "ordersOthersHaveBookedForMe": [
            {"orderCode": "B", "orderDate": "2026-11-02", "status": "BOOKED", "orderTotals": {"total": 5.0}},
            {"orderCode": "A", "orderDate": "2026-11-01", "status": "BOOKED", "orderTotals": {"total": 10.0}}
        ]
    }});
    let codes: Vec<String> = parse_order_summary(&v).unwrap().into_iter().map(|r| r.code).collect();
    assert_eq!(codes, ["A", "B"]);
}

#[test]
fn summary_without_my_orders_is_an_error() {
    assert!(parse_order_summary(&json!({"payload": {}})).is_err());
    assert!(parse_order_summary(&json!({})).is_err());
}

#[test]
fn package_detail_maps_fields() {
    let lines = parse_order_detail(&order("ID-5", "2026-11-05"), &fixture("order_detail_package.json"));
    assert_eq!(lines.len(), 2);
    let l = &lines[0];
    assert_eq!(l.order_code, "ID-5");
    assert_eq!(l.order_date, NaiveDate::from_ymd_opt(2026, 11, 5));
    assert_eq!(l.product_code, "3222");
    assert_eq!(l.category_prefix, "beverage");
    assert_eq!(l.title, "Deluxe Beverage Package");
    assert_eq!(l.unit, PaidUnit::Day);
    assert_eq!(l.subtotal, 482.93);
    assert_eq!(l.quantity, 1);
    assert_eq!(l.guest_type, GuestType::Adult);
    assert_eq!(l.guest_reservation_id.as_deref(), Some("ID-3"));
    assert_eq!(l.currency.as_deref(), Some("USD"));
    assert_ne!(lines[0].guest_id, lines[1].guest_id);
}

#[test]
fn cancelled_order_yields_nothing() {
    let lines = parse_order_detail(&order("ID-1", "2026-10-20"), &fixture("order_detail_cancelled.json"));
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn per_seat_is_each() {
    let lines = parse_order_detail(&order("ID-7", "2026-11-06"), &fixture("order_detail_oneoff.json"));
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].product_code, "Y6Y2");
    assert_eq!(lines[0].category_prefix, "shorex");
    assert_eq!(lines[0].unit, PaidUnit::Each);
    assert_eq!(lines[0].subtotal, 76.99);
}

#[test]
fn booked_variant_code_kept() {
    let lines = parse_order_detail(&order("ID-6", "2026-11-06"), &fixture("order_detail_variant.json"));
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].product_code, "33S2");
    assert_eq!(lines[0].category_prefix, "internet");
    assert_eq!(lines[0].unit, PaidUnit::Night);
    assert_eq!(lines[0].subtotal, 265.93);
}

fn item(guests: Value, product_summary: Value) -> Value {
    json!({"payload": {"status": "BOOKED", "orderHistoryDetailItems": [
        {"status": "BOOKED", "productSummary": product_summary, "guests": guests}
    ]}})
}

fn guest(id: &str, status: &str, subtotal: f64, ty: &str) -> Value {
    json!({"id": id, "orderStatus": status, "guestType": ty, "reservationId": "R1",
           "priceDetails": {"subtotal": subtotal, "quantity": 1, "currency": "USD"}})
}

fn summary(sales_unit: &str) -> Value {
    json!({"id": "BASE", "defaultVariantId": "VAR1", "title": "Thing",
           "productTypeCategory": {"id": "pt_shoreX"}, "salesUnit": sales_unit})
}

#[test]
fn cancelled_and_free_guests_skipped() {
    let v = item(
        json!([guest("g1", "CANCELLED", 50.0, "ADULT"), guest("g2", "BOOKED", 0.0, "ADULT"), guest("g3", "BOOKED", 50.0, "ADULT")]),
        summary("PER_SEAT"),
    );
    let lines = parse_order_detail(&order("O", "2026-11-01"), &v);
    assert_eq!(lines.iter().map(|l| l.guest_id.as_str()).collect::<Vec<_>>(), ["g3"]);
}

#[test]
fn cancelled_item_skipped() {
    let mut v = item(json!([guest("g1", "BOOKED", 50.0, "ADULT")]), summary("PER_SEAT"));
    v["payload"]["orderHistoryDetailItems"][0]["status"] = json!("CANCELLED");
    assert!(parse_order_detail(&order("O", "2026-11-01"), &v).is_empty());
}

#[test]
fn variant_code_falls_back_to_default_variant() {
    let v = item(json!([guest("g1", "BOOKED", 50.0, "ADULT")]), summary("PER_SEAT"));
    let lines = parse_order_detail(&order("O", "2026-11-01"), &v);
    assert_eq!(lines[0].product_code, "VAR1");
}

#[test]
fn sales_unit_mapping() {
    for (su, unit) in [
        ("PER_DAY", PaidUnit::Day),
        ("perDay", PaidUnit::Day),
        ("PER_NIGHT", PaidUnit::Night),
        ("perNight", PaidUnit::Night),
        ("PER_SEAT", PaidUnit::Each),
        ("perPackage", PaidUnit::Each),
    ] {
        let v = item(json!([guest("g1", "BOOKED", 50.0, "ADULT")]), summary(su));
        assert_eq!(parse_order_detail(&order("O", "2026-11-01"), &v)[0].unit, unit, "{su}");
    }
}

#[test]
fn guest_type_mapping() {
    let v = item(
        json!([guest("g1", "BOOKED", 50.0, "ADULT"), guest("g2", "BOOKED", 50.0, "CHILD")]),
        summary("PER_SEAT"),
    );
    let lines = parse_order_detail(&order("O", "2026-11-01"), &v);
    assert_eq!(lines[0].guest_type, GuestType::Adult);
    assert_eq!(lines[1].guest_type, GuestType::Child);
}
