use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

mod princess_decks;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Brand {
    Royal,
    Celebrity,
    Princess,
}

impl Brand {
    pub fn host(self) -> &'static str {
        match self {
            Brand::Royal => "www.royalcaribbean.com",
            Brand::Celebrity => "www.celebritycruises.com",
            Brand::Princess => "www.princess.com",
        }
    }

    pub fn api_host(self) -> &'static str {
        "aws-prd.api.rccl.com"
    }

    pub fn code(self) -> &'static str {
        match self {
            Brand::Royal => "R",
            Brand::Celebrity => "C",
            Brand::Princess => "P",
        }
    }

    pub fn url_segment(self) -> &'static str {
        match self {
            Brand::Royal => "royal",
            Brand::Celebrity => "celebrity",
            Brand::Princess => "princess",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Brand::Royal => "royal",
            Brand::Celebrity => "celebrity",
            Brand::Princess => "princess",
        }
    }

    /// Human-facing brand name.
    pub fn label(self) -> &'static str {
        match self {
            Brand::Royal => "Royal Caribbean",
            Brand::Celebrity => "Celebrity",
            Brand::Princess => "Princess",
        }
    }

    /// The cruise line (account family) this brand belongs to.
    pub fn line(self) -> CruiseLine {
        match self {
            Brand::Royal | Brand::Celebrity => CruiseLine::Rcg,
            Brand::Princess => CruiseLine::Princess,
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
            "princess" | "p" | "pcl" => Ok(Brand::Princess),
            _ => Err(ParseBrandError(s.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown brand: {0}")]
pub struct ParseBrandError(pub String);

/// A cruise line = one login system. Royal Caribbean and Celebrity share one
/// account (`Rcg`); Princess has its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CruiseLine {
    Rcg,
    Princess,
}

impl CruiseLine {
    pub fn as_str(self) -> &'static str {
        match self {
            CruiseLine::Rcg => "rcg",
            CruiseLine::Princess => "princess",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CruiseLine::Rcg => "Royal Caribbean & Celebrity",
            CruiseLine::Princess => "Princess",
        }
    }

    pub fn brands(self) -> &'static [Brand] {
        match self {
            CruiseLine::Rcg => &[Brand::Royal, Brand::Celebrity],
            CruiseLine::Princess => &[Brand::Princess],
        }
    }
}

impl std::fmt::Display for CruiseLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for CruiseLine {
    type Err = ParseCruiseLineError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "rcg" => Ok(CruiseLine::Rcg),
            "princess" => Ok(CruiseLine::Princess),
            _ => Err(ParseCruiseLineError(s.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown cruise line: {0}")]
pub struct ParseCruiseLineError(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub telegram_chat_id: i64,
    pub telegram_username: Option<String>,
    pub active: bool,
}

/// One set of cruise-line credentials belonging to a user. Deliberately not
/// `Serialize`: it carries the encrypted password (use a DTO for output).
#[derive(Clone)]
pub struct CruiseAccount {
    pub id: i64,
    pub user_id: i64,
    pub line: CruiseLine,
    pub login: String,
    pub password_ct: Vec<u8>,
    pub password_nonce: Vec<u8>,
    pub active: bool,
    pub last_login_at: Option<DateTime<Utc>>,
    pub last_login_error: Option<String>,
}

// Manual Debug so an account is never able to leak its encrypted password
// (or nonce) into logs.
impl std::fmt::Debug for CruiseAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CruiseAccount")
            .field("id", &self.id)
            .field("user_id", &self.user_id)
            .field("line", &self.line)
            .field("login", &self.login)
            .field("password_ct", &"<redacted>")
            .field("password_nonce", &"<redacted>")
            .field("active", &self.active)
            .field("last_login_at", &self.last_login_at)
            .field("last_login_error", &self.last_login_error)
            .finish()
    }
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
    /// Princess voyage code (e.g. `5543`); `None` for RCG bookings.
    #[serde(default)]
    pub voyage_code: Option<String>,
    /// ISO currency of the account's prices; `None` means USD.
    #[serde(default)]
    pub currency: Option<String>,
    /// Deck name when the line names its decks (Princess: `Dolphin`). RCG
    /// decks are numbered and derived from the cabin instead, so `None`.
    #[serde(default)]
    pub deck_name: Option<String>,
    /// Cabin category as the line describes it (Princess: `Deluxe Balcony`).
    #[serde(default)]
    pub cabin_category: Option<String>,
}

impl Booking {
    /// True when the booking is a guarantee cabin still showing "GTY".
    pub fn is_gty(&self) -> bool {
        self.stateroom.as_deref() == Some("GTY")
    }

    /// The ship has sailed (`today` is the UTC date). Add-ons can no longer be
    /// bought, so price checks, catalog refreshes and cabin lookups stop.
    pub fn has_departed(&self, today: NaiveDate) -> bool {
        self.sail_date <= today
    }

    /// The cruise is over: the day after disembarkation (sail date + nights) has
    /// arrived. Ended bookings are hidden. An unknown length counts as zero nights;
    /// one too long to add to the date (bad data) never ends rather than panicking.
    pub fn has_ended(&self, today: NaiveDate) -> bool {
        let nights = self.nights.and_then(|n| u64::try_from(n).ok()).unwrap_or(0);
        self.sail_date
            .checked_add_days(chrono::Days::new(nights))
            .is_some_and(|end| end < today)
    }
}

/// Map a brand + two-letter ship code to its full name. RCG and Princess reuse
/// codes for different ships (`ST`, `EX`), so the brand is part of the key. The
/// web Mini App gets ship names from the server rather than keeping its own
/// table. Returns `None` for unknown codes so callers can fall back to the raw code.
pub fn ship_name(brand: Brand, code: &str) -> Option<&'static str> {
    all_ships()
        .iter()
        .find(|(c, _, b)| *b == brand && *c == code)
        .map(|(_, name, _)| *name)
}

/// Every ship we know a name for, as `(code, name, brand)`, and the single
/// source for [`ship_name`]. Backs the public tier's ship picker so a user can
/// look up any sailing before anyone has tracked it.
///
/// RCG codes are the ones `aws-prd.api.rccl.com/en/{brand}/web/v3/ships/{code}/voyages`
/// answers to (one code space across Royal and Celebrity), confirmed 2026-10-26
/// by voyages plus deck counts. Princess codes come from `resdb/p1.0/ships`.
pub fn all_ships() -> &'static [(&'static str, &'static str, Brand)] {
    &[
        // Celebrity
        ("AT", "Celebrity Ascent", Brand::Celebrity),
        ("AX", "Celebrity Apex", Brand::Celebrity),
        ("BY", "Celebrity Beyond", Brand::Celebrity),
        ("CS", "Celebrity Constellation", Brand::Celebrity),
        ("EC", "Celebrity Eclipse", Brand::Celebrity),
        ("EG", "Celebrity Edge", Brand::Celebrity),
        ("EQ", "Celebrity Equinox", Brand::Celebrity),
        ("FL", "Celebrity Flora", Brand::Celebrity),
        ("IN", "Celebrity Infinity", Brand::Celebrity),
        ("ML", "Celebrity Millennium", Brand::Celebrity),
        ("RF", "Celebrity Reflection", Brand::Celebrity),
        ("SI", "Celebrity Silhouette", Brand::Celebrity),
        ("SL", "Celebrity Solstice", Brand::Celebrity),
        ("SM", "Celebrity Summit", Brand::Celebrity),
        ("XC", "Celebrity Xcel", Brand::Celebrity),
        // Royal Caribbean
        ("AD", "Adventure of the Seas", Brand::Royal),
        ("AL", "Allure of the Seas", Brand::Royal),
        ("AN", "Anthem of the Seas", Brand::Royal),
        ("BR", "Brilliance of the Seas", Brand::Royal),
        ("EN", "Enchantment of the Seas", Brand::Royal),
        ("EX", "Explorer of the Seas", Brand::Royal),
        ("FR", "Freedom of the Seas", Brand::Royal),
        ("GR", "Grandeur of the Seas", Brand::Royal),
        ("HM", "Harmony of the Seas", Brand::Royal),
        ("IC", "Icon of the Seas", Brand::Royal),
        ("ID", "Independence of the Seas", Brand::Royal),
        ("JW", "Jewel of the Seas", Brand::Royal),
        ("LB", "Liberty of the Seas", Brand::Royal),
        ("LE", "Legend of the Seas", Brand::Royal),
        ("MA", "Mariner of the Seas", Brand::Royal),
        ("NV", "Navigator of the Seas", Brand::Royal),
        ("OA", "Oasis of the Seas", Brand::Royal),
        ("OV", "Ovation of the Seas", Brand::Royal),
        ("OY", "Odyssey of the Seas", Brand::Royal),
        ("QN", "Quantum of the Seas", Brand::Royal),
        ("RD", "Radiance of the Seas", Brand::Royal),
        ("RH", "Rhapsody of the Seas", Brand::Royal),
        ("SC", "Spectrum of the Seas", Brand::Royal),
        ("SR", "Serenade of the Seas", Brand::Royal),
        ("ST", "Star of the Seas", Brand::Royal),
        ("SY", "Symphony of the Seas", Brand::Royal),
        ("UT", "Utopia of the Seas", Brand::Royal),
        ("VI", "Vision of the Seas", Brand::Royal),
        ("VY", "Voyager of the Seas", Brand::Royal),
        ("WN", "Wonder of the Seas", Brand::Royal),
        // Princess
        ("AP", "Grand Princess", Brand::Princess),
        ("CB", "Caribbean Princess", Brand::Princess),
        ("CO", "Coral Princess", Brand::Princess),
        ("DI", "Diamond Princess", Brand::Princess),
        ("EP", "Emerald Princess", Brand::Princess),
        ("EX", "Enchanted Princess", Brand::Princess),
        ("GP", "Regal Princess", Brand::Princess),
        ("IP", "Island Princess", Brand::Princess),
        ("KP", "Crown Princess", Brand::Princess),
        ("MJ", "Majestic Princess", Brand::Princess),
        ("RP", "Royal Princess", Brand::Princess),
        ("RU", "Ruby Princess", Brand::Princess),
        ("SA", "Sapphire Princess", Brand::Princess),
        ("ST", "Star Princess", Brand::Princess),
        ("SU", "Sun Princess", Brand::Princess),
        ("XP", "Discovery Princess", Brand::Princess),
        ("YP", "Sky Princess", Brand::Princess),
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
/// low/mid/high tier — deliberately not precise venue mapping. Princess ranges
/// come from the decks the Princess deck table marks as holding staterooms.
pub fn cabin_deck_range(brand: Brand, ship_code: &str) -> Option<(u16, u16)> {
    if brand == Brand::Princess {
        let mut cabin_decks = princess_decks::decks(ship_code)?
            .iter()
            .filter(|(_, _, cabins)| *cabins)
            .map(|(number, _, _)| *number);
        let lo = cabin_decks.next()?;
        return Some((lo, cabin_decks.last().unwrap_or(lo)));
    }
    match ship_code {
        // Oasis class
        "WN" | "SY" | "HM" | "OA" | "AL" | "UT" => Some((3, 18)),
        // Icon class
        "IC" | "ST" | "LE" => Some((5, 20)),
        // Quantum / Quantum-Ultra class
        "QN" | "AN" | "OV" | "SC" | "OY" => Some((3, 14)),
        // Celebrity Edge class
        "EG" | "AX" | "BY" | "AT" | "XC" => Some((3, 16)),
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

/// The deck number for a named deck (`Dolphin` -> 9 on most Princess ships),
/// matched case-insensitively. Only Princess names its decks; `None` for RCG and
/// for ships or names we have no table entry for.
pub fn named_deck_number(brand: Brand, ship_code: &str, deck_name: &str) -> Option<u16> {
    if brand != Brand::Princess {
        return None;
    }
    let name = deck_name.trim();
    princess_decks::decks(ship_code)?
        .iter()
        .find(|(_, n, _)| n.eq_ignore_ascii_case(name))
        .map(|(number, _, _)| *number)
}

/// Location facts for `cabin`. RCG cabin numbers encode the deck; Princess
/// cabins don't, so their deck comes from `deck_name` (as `booking/summary`
/// reports it) and is `None` without one.
pub fn cabin_location(
    brand: Brand,
    ship_code: &str,
    cabin: &str,
    deck_name: Option<&str>,
) -> Option<CabinLocation> {
    let deck = match brand {
        Brand::Princess => named_deck_number(brand, ship_code, deck_name?)?,
        Brand::Royal | Brand::Celebrity => deck_of_cabin(cabin)?,
    };
    let range = cabin_deck_range(brand, ship_code);
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
    pub voyage_code: Option<String>,
    pub currency: Option<String>,
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

/// Format a price for display. `None` or `"USD"` renders as `$24.99`; any other
/// currency as `AUD 24.94`.
pub fn format_money(amount: f64, currency: Option<&str>) -> String {
    match currency {
        None | Some("USD") => format!("${amount:.2}"),
        Some(cur) => format!("{cur} {amount:.2}"),
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
        assert_eq!(ship_name(Brand::Royal, "WN"), Some("Wonder of the Seas"));
        assert_eq!(ship_name(Brand::Celebrity, "AX"), Some("Celebrity Apex"));
        assert_eq!(ship_name(Brand::Royal, "ZZ"), None);
    }

    #[test]
    fn cabin_location_tiers() {
        // Wonder (Oasis, decks 3-18): 9434 is mid-ship height.
        let l = cabin_location(Brand::Royal, "WN", "9434", None).unwrap();
        assert_eq!(l.deck, 9);
        assert_eq!(l.tier, Some(HeightTier::Middle));
        assert!(!l.notes.is_empty());

        // Deck 3 is the lowest stateroom deck → Lower + waterline note.
        let low = cabin_location(Brand::Royal, "WN", "3218", None).unwrap();
        assert_eq!(low.tier, Some(HeightTier::Lower));
        assert!(low.notes.iter().any(|n| n.contains("waterline")));
    }

    #[test]
    fn cabin_location_unknown_ship_degrades_to_deck_only() {
        // Unknown ship code → deck derived, but no tier/range-based notes.
        let l = cabin_location(Brand::Royal, "ZZ", "9434", None).unwrap();
        assert_eq!(l.deck, 9);
        assert_eq!(l.tier, None);
        assert!(l.notes.is_empty());
    }

    #[test]
    fn cabin_location_none_for_gty() {
        assert!(cabin_location(Brand::Royal, "WN", "GTY", None).is_none());
    }

    #[test]
    fn cruise_line_roundtrip() {
        for l in [CruiseLine::Rcg, CruiseLine::Princess] {
            assert_eq!(l.as_str().parse::<CruiseLine>().unwrap(), l);
        }
        assert!("royal".parse::<CruiseLine>().is_err());
        assert_eq!(CruiseLine::Rcg.brands(), &[Brand::Royal, Brand::Celebrity]);
        assert_eq!(Brand::Princess.line(), CruiseLine::Princess);
        assert_eq!(Brand::Celebrity.line(), CruiseLine::Rcg);
        assert_eq!(CruiseLine::Rcg.label(), "Royal Caribbean & Celebrity");
        assert_eq!(CruiseLine::Princess.label(), "Princess");
    }

    #[test]
    fn brand_princess_parses() {
        assert_eq!("princess".parse::<Brand>().unwrap(), Brand::Princess);
        assert_eq!("pcl".parse::<Brand>().unwrap(), Brand::Princess);
        assert_eq!(Brand::Princess.to_string(), "princess");
        assert_eq!(Brand::Princess.host(), "www.princess.com");
        assert_eq!(Brand::Princess.code(), "P");
    }

    #[test]
    fn ship_codes_are_brand_qualified() {
        // RCG and Princess reuse codes for different ships.
        assert_eq!(ship_name(Brand::Royal, "ST"), Some("Star of the Seas"));
        assert_eq!(ship_name(Brand::Princess, "ST"), Some("Star Princess"));
        assert_eq!(ship_name(Brand::Royal, "EX"), Some("Explorer of the Seas"));
        assert_eq!(ship_name(Brand::Princess, "EX"), Some("Enchanted Princess"));
        assert_eq!(ship_name(Brand::Princess, "OA"), None);
    }

    #[test]
    fn rcg_ship_codes_match_the_voyages_api() {
        // Codes confirmed against /web/v3/ships/{code}/voyages + /decks (2026-10-26).
        assert_eq!(ship_name(Brand::Royal, "SR"), Some("Serenade of the Seas"));
        assert_eq!(ship_name(Brand::Royal, "SC"), Some("Spectrum of the Seas"));
        assert_eq!(ship_name(Brand::Royal, "OY"), Some("Odyssey of the Seas"));
        assert_eq!(ship_name(Brand::Royal, "VI"), Some("Vision of the Seas"));
        assert_eq!(ship_name(Brand::Royal, "ID"), Some("Independence of the Seas"));
        assert_eq!(ship_name(Brand::Royal, "LE"), Some("Legend of the Seas"));
        assert_eq!(ship_name(Brand::Celebrity, "SM"), Some("Celebrity Summit"));
        assert_eq!(ship_name(Brand::Celebrity, "EG"), Some("Celebrity Edge"));
        assert_eq!(ship_name(Brand::Celebrity, "BY"), Some("Celebrity Beyond"));
        assert_eq!(ship_name(Brand::Celebrity, "AT"), Some("Celebrity Ascent"));
        assert_eq!(ship_name(Brand::Celebrity, "ML"), Some("Celebrity Millennium"));
        // Codes that return no voyages are gone.
        for (brand, code) in [
            (Brand::Royal, "EP"),
            (Brand::Royal, "OD"),
            (Brand::Royal, "SP"),
            (Brand::Royal, "VS"),
            (Brand::Celebrity, "AS"),
            (Brand::Celebrity, "BE"),
            (Brand::Celebrity, "ED"),
            (Brand::Celebrity, "MI"),
            (Brand::Celebrity, "SU"),
        ] {
            assert_eq!(ship_name(brand, code), None, "{brand:?} {code}");
        }
        assert_eq!(cabin_deck_range(Brand::Royal, "ST"), Some((5, 20)));
        assert_eq!(cabin_deck_range(Brand::Royal, "SC"), Some((3, 14)));
        assert_eq!(cabin_deck_range(Brand::Celebrity, "EG"), Some((3, 16)));
        let count = |b: Brand| all_ships().iter().filter(|s| s.2 == b).count();
        assert_eq!(count(Brand::Royal), 30);
        assert_eq!(count(Brand::Celebrity), 15);
        let mut seen = std::collections::HashSet::new();
        for (code, _, brand) in all_ships() {
            assert!(seen.insert((*brand, *code)), "duplicate {brand:?} {code}");
        }
    }

    #[test]
    fn princess_ship_list_is_complete() {
        assert_eq!(
            all_ships()
                .iter()
                .filter(|s| s.2 == Brand::Princess)
                .count(),
            17
        );
        for (code, name, brand) in all_ships() {
            assert_eq!(ship_name(*brand, code), Some(*name));
        }
    }

    #[test]
    fn princess_deck_names_map_to_numbers() {
        assert_eq!(named_deck_number(Brand::Princess, "XP", "Dolphin"), Some(9));
        assert_eq!(named_deck_number(Brand::Princess, "XP", "DOLPHIN"), Some(9));
        // Upper decks differ by class: Lido is 16 on Royal-class, 14 on Grand-class.
        assert_eq!(named_deck_number(Brand::Princess, "XP", "Lido"), Some(16));
        assert_eq!(named_deck_number(Brand::Princess, "AP", "Lido"), Some(14));
        assert_eq!(named_deck_number(Brand::Princess, "XP", "Nowhere"), None);
        // No table for ships whose deck pages don't name their decks.
        assert_eq!(named_deck_number(Brand::Princess, "SU", "Dolphin"), None);
        assert_eq!(named_deck_number(Brand::Royal, "XP", "Dolphin"), None);
        assert_eq!(cabin_deck_range(Brand::Princess, "XP"), Some((8, 16)));
        assert_eq!(cabin_deck_range(Brand::Princess, "SU"), None);
    }

    #[test]
    fn princess_cabin_location_comes_from_the_deck_name() {
        // Discovery Princess staterooms span decks 8-16.
        let low = cabin_location(Brand::Princess, "XP", "D123", Some("Dolphin")).unwrap();
        assert_eq!(low.deck, 9);
        assert_eq!(low.tier, Some(HeightTier::Lower));
        let mid = cabin_location(Brand::Princess, "XP", "A123", Some("Aloha")).unwrap();
        assert_eq!((mid.deck, mid.tier), (12, Some(HeightTier::Middle)));
        // A Princess cabin number alone says nothing about the deck.
        assert!(cabin_location(Brand::Princess, "XP", "D123", None).is_none());
        assert!(cabin_location(Brand::Princess, "XP", "9434", None).is_none());
        // RCG ignores the deck name and reads the number.
        assert_eq!(cabin_location(Brand::Royal, "WN", "9434", Some("Dolphin")).unwrap().deck, 9);
    }

    fn booking_sailing(sail: (i32, u32, u32), nights: Option<i32>) -> Booking {
        Booking {
            reservation_id: "1234567".into(),
            brand: Brand::Royal,
            account_id: "a".into(),
            ship_code: "ST".into(),
            sail_date: NaiveDate::from_ymd_opt(sail.0, sail.1, sail.2).unwrap(),
            passenger_id: None,
            nights,
            package_code: None,
            stateroom: None,
            assigned_stateroom: None,
            voyage_code: None,
            currency: None,
            deck_name: None,
            cabin_category: None,
        }
    }

    #[test]
    fn departed_from_sail_day_on() {
        let d = |m, day| NaiveDate::from_ymd_opt(2027, m, day).unwrap();
        let b = booking_sailing((2027, 2, 7), Some(7));
        assert!(!b.has_departed(d(2, 6)));
        assert!(b.has_departed(d(2, 7)), "sail day itself is too late to buy add-ons");
        assert!(b.has_departed(d(2, 10)));
    }

    #[test]
    fn ended_the_day_after_disembarking() {
        let d = |m, day| NaiveDate::from_ymd_opt(2027, m, day).unwrap();
        let b = booking_sailing((2027, 2, 7), Some(7));
        assert!(!b.has_ended(d(2, 10)), "mid-cruise stays visible");
        assert!(!b.has_ended(d(2, 14)), "disembarkation day stays visible");
        assert!(b.has_ended(d(2, 15)));
        let unknown = booking_sailing((2027, 2, 7), None);
        assert!(!unknown.has_ended(d(2, 7)));
        assert!(unknown.has_ended(d(2, 8)));
        let negative = booking_sailing((2027, 2, 7), Some(-3));
        assert!(negative.has_ended(d(2, 8)), "nonsense nights count as zero");
        let absurd = booking_sailing((2027, 2, 7), Some(i32::MAX));
        assert!(!absurd.has_ended(d(2, 8)), "an overflowing length doesn't panic");
    }

    #[test]
    fn format_money_usd_and_aud() {
        assert_eq!(format_money(24.99, None), "$24.99");
        assert_eq!(format_money(24.99, Some("USD")), "$24.99");
        assert_eq!(format_money(24.94, Some("AUD")), "AUD 24.94");
    }

    #[test]
    fn cruise_account_debug_redacts_secrets() {
        let a = CruiseAccount {
            id: 7,
            user_id: 1,
            line: CruiseLine::Rcg,
            login: "me@example.com".into(),
            password_ct: vec![0xDE, 0xAD, 0xBE, 0xEF, 211, 199],
            password_nonce: vec![0xCA, 0xFE, 0xBA, 0xBE, 187, 173],
            active: true,
            last_login_at: None,
            last_login_error: None,
        };
        let s = format!("{a:?}");
        assert!(s.contains("<redacted>"));
        assert!(s.contains("me@example.com"));
        for needle in ["222", "173", "202", "254", "187", "211", "199", "190", "239"] {
            assert!(!s.contains(needle), "leaked byte {needle}: {s}");
        }
    }
}
