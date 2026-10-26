use base64::Engine;
use chrono::NaiveDate;
use royaltracker_api::princess::{
    basic_auth_value, parse_booking_summary, parse_departure, parse_keep_alive, summary_app_id, PrincessCabin,
};

fn fixture(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/fixtures/princess/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn basic_auth_urlencodes_both_parts() {
    let expected = base64::engine::general_purpose::STANDARD.encode("a%40b.com:p%231%21");
    assert_eq!(basic_auth_value("a@b.com", "p#1!"), format!("Basic {expected}"));
}

#[test]
fn basic_auth_encodes_space_like_encode_uri_component() {
    // The site uses encodeURIComponent: space -> %20 (never '+'), literal '+' -> %2B.
    let expected = base64::engine::general_purpose::STANDARD.encode("a%40b.com:p%20w%2B1");
    assert_eq!(basic_auth_value("a@b.com", "p w+1"), format!("Basic {expected}"));
}

#[test]
fn departure_text_parses() {
    assert_eq!(parse_departure("Saturday, October 24, 2026"), NaiveDate::from_ymd_opt(2026, 10, 24));
}

#[test]
fn keepalive_parses_context() {
    let c = parse_keep_alive(&fixture("keepalive.json")).unwrap();
    assert_eq!(c.booking_id, "AB12CD");
    assert_eq!(c.ship_code, "XP");
    assert_eq!(c.voyage_code, "X637");
    assert_eq!(c.pax_id, "1");
    assert_eq!(c.nights, Some(7));
    assert_eq!(c.sail_date, NaiveDate::from_ymd_opt(2026, 10, 24).unwrap());
    assert_eq!(c.currency, "USD");
    assert_eq!(c.cabin, None, "the cabin comes from booking/summary, not keepalive");
}

#[test]
fn keepalive_without_booking_info_is_none() {
    assert!(parse_keep_alive(&fixture("keepalive_no_booking.json")).is_none());
}

#[test]
fn keepalive_missing_field_or_bad_date_is_none() {
    let mut v = fixture("keepalive.json");
    v["data"]["bookingInfo"].as_object_mut().unwrap().remove("shipCode");
    assert!(parse_keep_alive(&v).is_none());

    let mut v = fixture("keepalive.json");
    v["data"]["bookingInfo"]["departureDate"] = serde_json::json!("2026-10-24");
    assert!(parse_keep_alive(&v).is_none());
}

#[test]
fn booking_summary_parses_cabin() {
    assert_eq!(
        parse_booking_summary(&fixture("booking_summary.json")),
        Some(PrincessCabin {
            number: "D123".into(),
            deck_name: Some("Dolphin".into()),
            category: Some("Deluxe Balcony - 2nd Sofabed".into()),
        })
    );
}

#[test]
fn booking_summary_without_a_cabin_is_none() {
    let mut v = fixture("booking_summary.json");
    v["booking"]["cabinNumber"] = serde_json::json!("");
    assert_eq!(parse_booking_summary(&v), None);
    assert_eq!(parse_booking_summary(&serde_json::json!({"segmentId": "0"})), None);

    let mut v = fixture("booking_summary.json");
    v["booking"].as_object_mut().unwrap().remove("deck");
    v["booking"]["categoryName"] = serde_json::json!(" ");
    let c = parse_booking_summary(&v).unwrap();
    assert_eq!((c.deck_name, c.category), (None, None));
}

#[test]
fn summary_app_id_names_the_booking() {
    let v: serde_json::Value = serde_json::from_str(&summary_app_id("00000000", "AB12CD")).unwrap();
    assert_eq!(v["bookNum"], "AB12CD");
    assert_eq!(v["agencyId"], "00000000");
    assert_eq!(v["cruiseLineCode"], "PCL");
    assert_eq!(v["systemId"], "PB");
}
