//! What a booking's cabin has already bought (Cruise Planner order history),
//! compared with today's price.

use royaltracker_types::Brand;

/// One booking as one account sees it: everything needed to read that
/// account's order history for the booking. Order history is relative to the
/// account asking, so a booking shared by two accounts yields two targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderTarget {
    pub brand: Brand,
    pub reservation_id: String,
    pub ship_code: String,
    /// Sail date in the wire format `YYYYMMDD`.
    pub sail_wire: String,
    /// This account's passenger id on the booking.
    pub passenger_id: String,
}
