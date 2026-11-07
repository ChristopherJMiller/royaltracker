//! Strip people and identifiers out of raw RCG order-history JSON so a real
//! response can be inspected or committed as a test fixture. Product codes,
//! categories, prices and statuses are kept.

use serde_json::Value;
use std::collections::HashMap;

/// Keys whose values (strings or numbers) are personal details: replaced with
/// "REDACTED".
const PERSONAL_KEYS: &[&str] = &[
    "firstName",
    "lastName",
    "middleName",
    "preferredName",
    "fullName",
    "email",
    "emailAddress",
    "phone",
    "phoneNumber",
    "birthDate",
    "dob",
    "age",
    "dateOfBirth",
    "loyaltyNumber",
    "stateroomNumber",
    "addressLine1",
    "addressLine2",
];

/// Keys whose values are account/booking identifiers: replaced with stable
/// aliases so equal ids stay equal (needed to follow guests across orders).
const ID_KEYS: &[&str] = &[
    "orderCode",
    "reservationId",
    "bookingId",
    "passengerId",
    "masterPassengerId",
    "guestId",
];

/// Scrub `v` in place. See the module docs for what is kept.
pub fn scrub_json(v: &mut Value) {
    let mut aliases = HashMap::new();
    walk(v, &mut aliases);
}

fn walk(v: &mut Value, aliases: &mut HashMap<String, String>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                if PERSONAL_KEYS.contains(&k.as_str()) && (val.is_string() || val.is_number()) {
                    *val = Value::String("REDACTED".into());
                } else if let Some(s) = id_value(k, val) {
                    *val = Value::String(alias(&s, aliases));
                } else {
                    walk(val, aliases);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| walk(i, aliases)),
        Value::String(s) if is_sailing_id(s) => *s = "XX20990101".into(),
        _ => {}
    }
}

/// The identifier to alias, if `key`/`val` is one. A bare `id` only counts when
/// it looks like a passenger id (all digits, 6+), so product ids survive.
fn id_value(key: &str, val: &Value) -> Option<String> {
    let s = match val {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let is_id = ID_KEYS.contains(&key)
        || (key == "id" && s.len() >= 6 && s.bytes().all(|b| b.is_ascii_digit()));
    is_id.then_some(s)
}

fn alias(real: &str, aliases: &mut HashMap<String, String>) -> String {
    let next = aliases.len() + 1;
    aliases
        .entry(real.to_string())
        .or_insert_with(|| format!("ID-{next}"))
        .clone()
}

/// Ship code + sail date, e.g. `QQ20301225`.
fn is_sailing_id(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[..2].iter().all(|c| c.is_ascii_uppercase())
        && b[2..].iter().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scrubs_people_and_ids_but_keeps_products() {
        let mut v = json!({"payload": {"myOrders": [{"orderCode": "AB12CD", "orderDate": "2026-09-12"}],
          "orderHistoryDetailItems": [{"productSummary": {"id": "3222", "baseOptions": [{"selected": {"code": "3222"}}]},
            "guests": [{"id": "11112222", "firstName": "Pat", "lastName": "Doe", "reservationId": "1234567",
                        "stateroomNumber": "1234", "guestType": "ADULT", "sailingId": "QQ20301225",
                        "dob": "1990-01-02", "age": 36},
                       {"id": "11112222", "firstName": "Pat"}]}]}});
        scrub_json(&mut v);
        let g = &v["payload"]["orderHistoryDetailItems"][0]["guests"];
        assert_eq!(g[0]["firstName"], "REDACTED");
        assert_eq!(g[0]["lastName"], "REDACTED");
        assert_eq!(g[0]["stateroomNumber"], "REDACTED");
        assert_eq!(g[0]["dob"], "REDACTED");
        assert_eq!(g[0]["age"], "REDACTED");
        assert_ne!(g[0]["id"], "11112222");
        assert_eq!(g[0]["id"], g[1]["id"]); // same real id → same alias
        assert_ne!(g[0]["reservationId"], "1234567");
        assert_eq!(g[0]["sailingId"], "XX20990101");
        assert_ne!(v["payload"]["myOrders"][0]["orderCode"], "AB12CD");
        assert_eq!(g[0]["guestType"], "ADULT");
        assert_eq!(
            v["payload"]["orderHistoryDetailItems"][0]["productSummary"]["id"],
            "3222"
        ); // product codes kept
    }
}
