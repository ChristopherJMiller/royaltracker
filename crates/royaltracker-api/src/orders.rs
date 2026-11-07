//! Cruise Planner order history (what a booking has already bought), parsed
//! into one line per guest per purchased product.

use crate::client::CruiseClient;
use crate::error::ApiError;
use chrono::NaiveDate;
use royaltracker_types::{Brand, GuestType, PaidUnit};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

const CANCELLED: &str = "CANCELLED";

/// One order in the order-history summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderRef {
    pub code: String,
    pub date: Option<NaiveDate>,
}

/// One guest's purchase of one product within one order.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderLine {
    pub order_code: String,
    pub order_date: Option<NaiveDate>,
    pub guest_id: String,
    /// The reservation the guest belongs to; shared orders can list guests
    /// from other cabins. `None` when the response doesn't say.
    pub guest_reservation_id: Option<String>,
    pub guest_type: GuestType,
    pub category_prefix: String,
    /// The variant bought (`baseOptions[0].selected.code`, else the default
    /// variant id).
    pub product_code: String,
    pub title: String,
    pub unit: PaidUnit,
    /// What this guest paid for the whole line (all days, all quantity).
    pub subtotal: f64,
    /// 0 when the response doesn't say.
    pub quantity: u32,
    pub currency: Option<String>,
}

fn str_at<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

/// A JSON id as a string, whether the wire sent a string or a number.
fn id_at(v: &Value, ptr: &str) -> Option<String> {
    match v.pointer(ptr)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn date_at(v: &Value, ptr: &str) -> Option<NaiveDate> {
    str_at(v, ptr).and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
}

/// The orders worth reading in an order-history summary: what this account
/// bought plus what others bought for it, minus cancelled and $0 orders. A
/// cancelled order keeps its total, so its status is what marks it. A
/// response without `payload.myOrders` is an error, never "nothing booked".
pub fn parse_order_summary(v: &Value) -> Result<Vec<OrderRef>, ApiError> {
    let mine = v
        .pointer("/payload/myOrders")
        .and_then(Value::as_array)
        .ok_or(ApiError::MissingField("payload.myOrders"))?;
    let others = v
        .pointer("/payload/ordersOthersHaveBookedForMe")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for o in mine.iter().chain(others) {
        let Some(code) = str_at(o, "/orderCode") else {
            continue;
        };
        let cancelled = str_at(o, "/status") == Some(CANCELLED);
        let free = o.pointer("/orderTotals/total").and_then(Value::as_f64) == Some(0.0);
        if cancelled || free || !seen.insert(code.to_string()) {
            continue;
        }
        out.push(OrderRef {
            code: code.to_string(),
            date: date_at(o, "/orderDate"),
        });
    }
    Ok(out)
}

/// Catalog categories are `beverage`, `shorex`…; order history says
/// `pt_beverage`, `pt_shoreX`.
fn catalog_category(order_category: &str) -> String {
    order_category
        .strip_prefix("pt_")
        .unwrap_or(order_category)
        .to_ascii_lowercase()
}

/// `PER_DAY` / `perDay` → day, `PER_NIGHT` / `perNight` → night, anything else
/// (`PER_SEAT`, `perPackage`…) is a one-off.
fn paid_unit(sales_unit: Option<&str>) -> PaidUnit {
    let s = sales_unit.unwrap_or_default().replace('_', "").to_ascii_lowercase();
    match s.as_str() {
        "perday" => PaidUnit::Day,
        "pernight" => PaidUnit::Night,
        _ => PaidUnit::Each,
    }
}

/// One line per guest per purchased product in an order's detail. Cancelled
/// orders, items and guests are skipped (a cancelled guest keeps its old
/// subtotal), as are $0 guests (included perks).
pub fn parse_order_detail(order: &OrderRef, v: &Value) -> Vec<OrderLine> {
    if str_at(v, "/payload/status") == Some(CANCELLED) {
        return Vec::new();
    }
    let Some(items) = v.pointer("/payload/orderHistoryDetailItems").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        if str_at(item, "/status") == Some(CANCELLED) {
            continue;
        }
        let ps = &item["productSummary"];
        let Some(code) = str_at(ps, "/baseOptions/0/selected/code").or_else(|| str_at(ps, "/defaultVariantId"))
        else {
            continue;
        };
        let category = str_at(ps, "/productTypeCategory/id").map(catalog_category).unwrap_or_default();
        let unit = paid_unit(str_at(ps, "/salesUnit"));
        let title = str_at(ps, "/title").unwrap_or_default();
        for g in item.get("guests").and_then(Value::as_array).into_iter().flatten() {
            let subtotal = g.pointer("/priceDetails/subtotal").and_then(Value::as_f64).unwrap_or(0.0);
            if str_at(g, "/orderStatus") == Some(CANCELLED) || subtotal <= 0.0 {
                continue;
            }
            let Some(guest_id) = id_at(g, "/id") else {
                continue;
            };
            out.push(OrderLine {
                order_code: order.code.clone(),
                order_date: order.date,
                guest_id,
                guest_reservation_id: id_at(g, "/reservationId"),
                guest_type: if str_at(g, "/guestType") == Some("ADULT") {
                    GuestType::Adult
                } else {
                    GuestType::Child
                },
                category_prefix: category.clone(),
                product_code: code.to_string(),
                title: title.to_string(),
                unit,
                subtotal,
                quantity: g
                    .pointer("/priceDetails/quantity")
                    .and_then(Value::as_u64)
                    .and_then(|q| u32::try_from(q).ok())
                    .unwrap_or(0),
                currency: str_at(g, "/priceDetails/currency").map(str::to_owned),
            });
        }
    }
    out
}

impl CruiseClient {
    /// Everything this account can see bought on a booking, one line per guest
    /// per product. Sleeps `gap` before each order-detail call. Any failed or
    /// malformed response fails the whole read: a partial read never comes
    /// back, so a caller can't mistake it for "these were cancelled".
    pub async fn fetch_order_lines(
        &self,
        brand: Brand,
        ship_code: &str,
        sail_wire: &str,
        passenger_id: &str,
        reservation_id: &str,
        gap: Duration,
    ) -> Result<Vec<OrderLine>, ApiError> {
        let summary = self
            .fetch_order_history(brand, ship_code, sail_wire, passenger_id, reservation_id)
            .await?;
        let mut lines = Vec::new();
        for order in parse_order_summary(&summary)? {
            tokio::time::sleep(gap).await;
            let detail = self
                .fetch_order_detail(brand, ship_code, sail_wire, &order.code, passenger_id, reservation_id)
                .await?;
            if detail.pointer("/payload/orderHistoryDetailItems").and_then(Value::as_array).is_none() {
                return Err(ApiError::MissingField("payload.orderHistoryDetailItems"));
            }
            lines.extend(parse_order_detail(&order, &detail));
        }
        Ok(lines)
    }
}
