use chrono::NaiveDate;
use royaltracker_api::princess::parse_schedule;

fn fixture(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/fixtures/princess/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn schedule_parses_combo_voyages() {
    let v = parse_schedule(&fixture("schedule_sample.json"));
    let st: Vec<_> = v
        .iter()
        .filter(|x| x.ship_code == "ST" && x.sail_date == NaiveDate::from_ymd_opt(2027, 1, 2).unwrap())
        .collect();
    assert_eq!(st.len(), 2);
    assert!(st.iter().any(|x| x.voyage_code == "4702" && x.nights == 7));
    assert!(st.iter().any(|x| x.voyage_code == "4702A" && x.nights == 14));
    assert!(st.iter().all(|x| !x.itinerary.is_empty()));
}

#[test]
fn schedule_currency_from_ship_currency() {
    let v = parse_schedule(&fixture("schedule_sample.json"));
    assert!(v.iter().any(|x| x.currency == "AUD"));
    assert!(v.iter().any(|x| x.currency == "USD"));
    assert!(v.iter().all(|x| !x.currency.is_empty()));
}

#[test]
fn schedule_is_sorted() {
    let v = parse_schedule(&fixture("schedule_sample.json"));
    let keys: Vec<_> = v.iter().map(|x| (x.ship_code.clone(), x.sail_date, x.voyage_code.clone())).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}
