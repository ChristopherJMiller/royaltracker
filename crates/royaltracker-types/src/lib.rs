use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Brand {
    Royal,
    Celebrity,
}

impl Brand {
    pub fn host(self) -> &'static str {
        match self {
            Brand::Royal => "www.royalcaribbean.com",
            Brand::Celebrity => "www.celebritycruises.com",
        }
    }

    pub fn api_host(self) -> &'static str {
        "aws-prd.api.rccl.com"
    }

    pub fn code(self) -> &'static str {
        match self {
            Brand::Royal => "R",
            Brand::Celebrity => "C",
        }
    }

    pub fn url_segment(self) -> &'static str {
        match self {
            Brand::Royal => "royal",
            Brand::Celebrity => "celebrity",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Brand::Royal => "royal",
            Brand::Celebrity => "celebrity",
        }
    }
}

impl std::fmt::Display for Brand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Brand {
    type Err = ParseBrandError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "royal" | "r" | "rcl" => Ok(Brand::Royal),
            "celebrity" | "c" | "x" => Ok(Brand::Celebrity),
            _ => Err(ParseBrandError(s.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown brand: {0}")]
pub struct ParseBrandError(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub telegram_chat_id: i64,
    pub telegram_username: Option<String>,
    pub rcg_username: String,
    pub rcg_password_ct: Vec<u8>,
    pub rcg_password_nonce: Vec<u8>,
    pub brand_pref: Brand,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Booking {
    pub reservation_id: String,
    pub brand: Brand,
    pub account_id: String,
    pub ship_code: String,
    pub sail_date: NaiveDate,
    pub passenger_id: Option<String>,
    pub nights: Option<i32>,
    pub package_code: Option<String>,
    /// Stateroom as RCG *displays* it: a real cabin number once assigned, or the
    /// literal `"GTY"` while a guarantee cabin is still unassigned. `None` when
    /// we haven't discovered it yet.
    #[serde(default)]
    pub stateroom: Option<String>,
    /// The physical cabin a `"GTY"` booking is *currently* assigned to
    /// internally, recovered from purchased add-on order records (the room leaks
    /// through the guest records on excursions/dining even while the booking
    /// still shows "GTY"). `Some` only when [`Self::stateroom`] is `"GTY"` and at
    /// least one order leaked the room; `None` otherwise.
    #[serde(default)]
    pub assigned_stateroom: Option<String>,
}

impl Booking {
    /// True when the booking is a guarantee cabin still showing "GTY".
    pub fn is_gty(&self) -> bool {
        self.stateroom.as_deref() == Some("GTY")
    }
}

/// Map a two-letter ship code to its full name. Kept in sync with the `SHIP_NAMES`
/// table in the web Mini App (`static/app.js`). Returns `None` for unknown codes
/// so callers can fall back to the raw code.
pub fn ship_name(code: &str) -> Option<&'static str> {
    let name = match code {
        // Celebrity
        "AS" => "Celebrity Ascent",
        "AX" => "Celebrity Apex",
        "BE" => "Celebrity Beyond",
        "EC" => "Celebrity Eclipse",
        "ED" => "Celebrity Edge",
        "EQ" => "Celebrity Equinox",
        "IN" => "Celebrity Infinity",
        "MI" => "Celebrity Millennium",
        "RF" => "Celebrity Reflection",
        "SI" => "Celebrity Silhouette",
        "SL" => "Celebrity Solstice",
        "SU" => "Celebrity Summit",
        "XC" => "Celebrity Xcel",
        // Royal Caribbean
        "AD" => "Adventure of the Seas",
        "AL" => "Allure of the Seas",
        "AN" => "Anthem of the Seas",
        "BR" => "Brilliance of the Seas",
        "EN" => "Enchantment of the Seas",
        "EP" => "Explorer of the Seas",
        "FR" => "Freedom of the Seas",
        "GR" => "Grandeur of the Seas",
        "HM" => "Harmony of the Seas",
        "IC" => "Icon of the Seas",
        "JW" => "Jewel of the Seas",
        "LB" => "Liberty of the Seas",
        "MA" => "Mariner of the Seas",
        "NV" => "Navigator of the Seas",
        "OA" => "Oasis of the Seas",
        "OD" => "Odyssey of the Seas",
        "OV" => "Ovation of the Seas",
        "QN" => "Quantum of the Seas",
        "RD" => "Radiance of the Seas",
        "RH" => "Rhapsody of the Seas",
        "SP" => "Spectrum of the Seas",
        "SR" => "Star of the Seas",
        "SY" => "Symphony of the Seas",
        "UT" => "Utopia of the Seas",
        "VS" => "Vision of the Seas",
        "VY" => "Voyager of the Seas",
        "WN" => "Wonder of the Seas",
        _ => return None,
    };
    Some(name)
}

/// Every ship we know a name for, as `(code, name, brand)`. Backs the public
/// tier's ship picker so a user can look up any sailing before anyone has tracked
/// it. Keep in sync with `ship_name`.
pub fn all_ships() -> &'static [(&'static str, &'static str, Brand)] {
    &[
        // Celebrity
        ("AS", "Celebrity Ascent", Brand::Celebrity),
        ("AX", "Celebrity Apex", Brand::Celebrity),
        ("BE", "Celebrity Beyond", Brand::Celebrity),
        ("EC", "Celebrity Eclipse", Brand::Celebrity),
        ("ED", "Celebrity Edge", Brand::Celebrity),
        ("EQ", "Celebrity Equinox", Brand::Celebrity),
        ("IN", "Celebrity Infinity", Brand::Celebrity),
        ("MI", "Celebrity Millennium", Brand::Celebrity),
        ("RF", "Celebrity Reflection", Brand::Celebrity),
        ("SI", "Celebrity Silhouette", Brand::Celebrity),
        ("SL", "Celebrity Solstice", Brand::Celebrity),
        ("SU", "Celebrity Summit", Brand::Celebrity),
        ("XC", "Celebrity Xcel", Brand::Celebrity),
        // Royal Caribbean
        ("AD", "Adventure of the Seas", Brand::Royal),
        ("AL", "Allure of the Seas", Brand::Royal),
        ("AN", "Anthem of the Seas", Brand::Royal),
        ("BR", "Brilliance of the Seas", Brand::Royal),
        ("EN", "Enchantment of the Seas", Brand::Royal),
        ("EP", "Explorer of the Seas", Brand::Royal),
        ("FR", "Freedom of the Seas", Brand::Royal),
        ("GR", "Grandeur of the Seas", Brand::Royal),
        ("HM", "Harmony of the Seas", Brand::Royal),
        ("IC", "Icon of the Seas", Brand::Royal),
        ("JW", "Jewel of the Seas", Brand::Royal),
        ("LB", "Liberty of the Seas", Brand::Royal),
        ("MA", "Mariner of the Seas", Brand::Royal),
        ("NV", "Navigator of the Seas", Brand::Royal),
        ("OA", "Oasis of the Seas", Brand::Royal),
        ("OD", "Odyssey of the Seas", Brand::Royal),
        ("OV", "Ovation of the Seas", Brand::Royal),
        ("QN", "Quantum of the Seas", Brand::Royal),
        ("RD", "Radiance of the Seas", Brand::Royal),
        ("RH", "Rhapsody of the Seas", Brand::Royal),
        ("SP", "Spectrum of the Seas", Brand::Royal),
        ("SR", "Star of the Seas", Brand::Royal),
        ("SY", "Symphony of the Seas", Brand::Royal),
        ("UT", "Utopia of the Seas", Brand::Royal),
        ("VS", "Vision of the Seas", Brand::Royal),
        ("VY", "Voyager of the Seas", Brand::Royal),
        ("WN", "Wonder of the Seas", Brand::Royal),
    ]
}

/// Best-effort deck number for a Royal Caribbean / Celebrity cabin number.
///
/// Modern ships encode the deck as the leading digit(s) followed by a 3-digit
/// position within the deck (`D` + `NNN`): `9434` → deck 9, `3218` → deck 3,
/// `10601` → deck 10, `12169` → deck 12. Returns `None` for the "GTY"
/// placeholder, non-numeric input, anything shorter than 4 digits (older 3-digit
/// schemes are ambiguous), or an implausible deck (>25).
pub fn deck_of_cabin(cabin: &str) -> Option<u16> {
    let c = cabin.trim();
    if c.len() < 4 || !c.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    c[..c.len() - 3]
        .parse::<u16>()
        .ok()
        .filter(|d| (1..=25).contains(d))
}

/// Relative height of a cabin within its ship's stateroom decks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeightTier {
    Lower,
    Middle,
    Upper,
}

impl HeightTier {
    pub fn label(self) -> &'static str {
        match self {
            HeightTier::Lower => "lower decks",
            HeightTier::Middle => "mid-ship height",
            HeightTier::Upper => "upper decks",
        }
    }
}

/// The stateroom deck range `(lowest, highest)` for a ship, by class. Only the
/// classes we've confirmed are listed; unknown ships return `None` and callers
/// degrade to deck-only. Ranges are coarse public facts used solely for a
/// low/mid/high tier — deliberately not precise venue mapping.
pub fn cabin_deck_range(ship_code: &str) -> Option<(u16, u16)> {
    match ship_code {
        // Oasis class
        "WN" | "SY" | "HM" | "OA" | "AL" | "UT" => Some((3, 18)),
        // Icon class
        "IC" | "SR" => Some((5, 20)),
        // Quantum / Quantum-Ultra class
        "QN" | "AN" | "OV" | "SP" | "OD" => Some((3, 14)),
        // Celebrity Edge class
        "ED" | "AX" | "BE" | "AS" | "XC" => Some((3, 16)),
        _ => None,
    }
}

/// Derived, low-risk location facts for a cabin: its deck, a height tier (when
/// we know the ship's deck range), and a few hedged human notes. Intentionally
/// conservative — it never asserts a specific venue is above/below (that's the
/// deck-plan view's job).
#[derive(Debug, Clone)]
pub struct CabinLocation {
    pub deck: u16,
    pub tier: Option<HeightTier>,
    pub notes: Vec<String>,
}

pub fn cabin_location(ship_code: &str, cabin: &str) -> Option<CabinLocation> {
    let deck = deck_of_cabin(cabin)?;
    let range = cabin_deck_range(ship_code);
    let tier = range.map(|(lo, hi)| {
        let span = hi.saturating_sub(lo).max(1);
        if deck <= lo + span / 3 {
            HeightTier::Lower
        } else if deck + span / 3 >= hi {
            HeightTier::Upper
        } else {
            HeightTier::Middle
        }
    });

    let mut notes = Vec::new();
    match tier {
        Some(HeightTier::Upper) => {
            notes.push("Higher decks feel more ship motion in rough seas.".to_string())
        }
        Some(HeightTier::Lower) => {
            notes.push("Lower decks feel the least ship motion.".to_string())
        }
        Some(HeightTier::Middle) => {
            notes.push("Mid-height decks are a good motion compromise.".to_string())
        }
        None => {}
    }
    if let Some((lo, hi)) = range {
        if deck >= hi {
            notes.push(
                "Top stateroom deck — check the deck above (often a pool, sports, or buffet deck) for early-morning noise."
                    .to_string(),
            );
        } else if deck <= lo {
            notes.push(
                "Lowest stateroom deck — close to the waterline and usually steady.".to_string(),
            );
        }
    }

    Some(CabinLocation { deck, tier, notes })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertMode {
    AnyDrop,
    BelowThreshold,
}

impl AlertMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertMode::AnyDrop => "any_drop",
            AlertMode::BelowThreshold => "below_threshold",
        }
    }
}

impl std::fmt::Display for AlertMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for AlertMode {
    type Err = ParseAlertModeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "any_drop" => Ok(AlertMode::AnyDrop),
            "below_threshold" => Ok(AlertMode::BelowThreshold),
            _ => Err(ParseAlertModeError(s.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown alert mode: {0}")]
pub struct ParseAlertModeError(pub String);

/// Delivery channel for a public-tier (no-login) subscription. Telegram is NOT
/// a member — authed Telegram targets are synthesized from `users` at query
/// time, not stored in the public-channel table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicChannelKind {
    WebPush,
    Email,
}

impl PublicChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PublicChannelKind::WebPush => "webpush",
            PublicChannelKind::Email => "email",
        }
    }
}

impl std::fmt::Display for PublicChannelKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for PublicChannelKind {
    type Err = ParsePublicChannelKindError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "webpush" => Ok(PublicChannelKind::WebPush),
            "email" => Ok(PublicChannelKind::Email),
            _ => Err(ParsePublicChannelKindError(s.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown public channel kind: {0}")]
pub struct ParsePublicChannelKindError(pub String);

/// A distinct sailing (brand + ship + departure date) — the identity the public
/// tier keys price series on. One fetch of a sailing's public prices serves every
/// subscriber and, via convergence, the owner's booking display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sailing {
    pub id: i64,
    pub brand: Brand,
    pub ship_code: String,
    pub sail_date: NaiveDate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchedProduct {
    pub id: i64,
    pub reservation_id: String,
    pub category_prefix: String,
    pub product_code: String,
    pub label: Option<String>,
    pub active: bool,
    pub alert_mode: AlertMode,
    pub alert_threshold: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceSnapshot {
    pub watched_id: i64,
    pub fetched_at: DateTime<Utc>,
    pub adult_promo_price: Option<f64>,
    pub child_promo_price: Option<f64>,
    pub raw_response: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diff {
    pub watched_id: i64,
    pub detected_at: DateTime<Utc>,
    pub old_price: f64,
    pub new_price: f64,
    pub delta_pct: f64,
    pub notified: bool,
}

impl Diff {
    pub fn from_prices(watched_id: i64, old: f64, new: f64) -> Self {
        let delta_pct = if old > 0.0 {
            ((new - old) / old) * 100.0
        } else {
            0.0
        };
        Self {
            watched_id,
            detected_at: Utc::now(),
            old_price: old,
            new_price: new,
            delta_pct,
            notified: false,
        }
    }

    pub fn is_drop(&self) -> bool {
        self.new_price < self.old_price
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_from_real_cabins() {
        assert_eq!(deck_of_cabin("9434"), Some(9)); // 3-night interior
        assert_eq!(deck_of_cabin("10601"), Some(10)); // 4-night balcony
        assert_eq!(deck_of_cabin("3218"), Some(3)); // friends' oceanview
        assert_eq!(deck_of_cabin("12169"), Some(12)); // Celebrity Apex balcony
    }

    #[test]
    fn deck_rejects_non_cabins() {
        assert_eq!(deck_of_cabin("GTY"), None);
        assert_eq!(deck_of_cabin(""), None);
        assert_eq!(deck_of_cabin("12"), None); // too short to disambiguate
        assert_eq!(deck_of_cabin("12A"), None); // non-numeric
        assert_eq!(deck_of_cabin("99999"), None); // deck 99 implausible
    }

    #[test]
    fn deck_trims_whitespace() {
        assert_eq!(deck_of_cabin(" 9434 "), Some(9));
    }

    #[test]
    fn ship_name_known_and_unknown() {
        assert_eq!(ship_name("WN"), Some("Wonder of the Seas"));
        assert_eq!(ship_name("AX"), Some("Celebrity Apex"));
        assert_eq!(ship_name("ZZ"), None);
    }

    #[test]
    fn cabin_location_tiers() {
        // Wonder (Oasis, decks 3-18): 9434 is mid-ship height.
        let l = cabin_location("WN", "9434").unwrap();
        assert_eq!(l.deck, 9);
        assert_eq!(l.tier, Some(HeightTier::Middle));
        assert!(!l.notes.is_empty());

        // Deck 3 is the lowest stateroom deck → Lower + waterline note.
        let low = cabin_location("WN", "3218").unwrap();
        assert_eq!(low.tier, Some(HeightTier::Lower));
        assert!(low.notes.iter().any(|n| n.contains("waterline")));
    }

    #[test]
    fn cabin_location_unknown_ship_degrades_to_deck_only() {
        // Unknown ship code → deck derived, but no tier/range-based notes.
        let l = cabin_location("ZZ", "9434").unwrap();
        assert_eq!(l.deck, 9);
        assert_eq!(l.tier, None);
        assert!(l.notes.is_empty());
    }

    #[test]
    fn cabin_location_none_for_gty() {
        assert!(cabin_location("WN", "GTY").is_none());
    }
}
