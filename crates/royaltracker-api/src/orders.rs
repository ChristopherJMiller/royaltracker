//! Cruise Planner order history (what a booking has already bought), parsed
//! into one line per guest per purchased product.

use chrono::NaiveDate;
use royaltracker_types::{GuestType, PaidUnit};

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
