use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use royaltracker_types::{
    AlertMode, Booking, Brand, Diff, PriceSnapshot, PublicChannelKind, Sailing, User, WatchedProduct,
};

#[derive(Debug, Clone)]
pub struct NewUser<'a> {
    pub telegram_chat_id: i64,
    pub telegram_username: Option<&'a str>,
    pub rcg_username: &'a str,
    pub rcg_password_ct: &'a [u8],
    pub rcg_password_nonce: &'a [u8],
    pub brand_pref: Brand,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("database: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("not found")]
    NotFound,
}

#[async_trait]
pub trait PriceRepo: Send + Sync + 'static {
    async fn migrate(&self) -> Result<(), StorageError>;

    // --- users ---
    async fn upsert_user(&self, u: &NewUser<'_>) -> Result<i64, StorageError>;
    async fn get_user_by_chat_id(&self, chat_id: i64) -> Result<Option<User>, StorageError>;
    async fn list_active_users(&self) -> Result<Vec<User>, StorageError>;
    async fn deactivate_user(&self, chat_id: i64) -> Result<(), StorageError>;
    async fn set_user_brand(&self, chat_id: i64, brand: Brand) -> Result<(), StorageError>;

    async fn upsert_booking(&self, booking: &Booking) -> Result<(), StorageError>;
    async fn list_bookings(&self) -> Result<Vec<Booking>, StorageError>;
    async fn list_bookings_for_user(&self, user_id: i64) -> Result<Vec<Booking>, StorageError>;
    async fn subscribe_user_to_booking(
        &self,
        reservation_id: &str,
        user_id: i64,
    ) -> Result<(), StorageError>;
    async fn user_owns_reservation(
        &self,
        user_id: i64,
        reservation_id: &str,
    ) -> Result<bool, StorageError>;

    /// Everyone subscribed to a given reservation. Used by the scraper to fan
    /// out a single price-drop notification to all subscribers, and by the web
    /// UI to show a "shared with" badge on the booking page.
    async fn list_subscribers_for_reservation(
        &self,
        reservation_id: &str,
    ) -> Result<Vec<SubscriberInfo>, StorageError>;

    async fn upsert_watched(
        &self,
        reservation_id: &str,
        category_prefix: &str,
        product_code: &str,
        label: Option<&str>,
    ) -> Result<i64, StorageError>;

    async fn set_watch_alert(
        &self,
        watched_id: i64,
        mode: AlertMode,
        threshold: Option<f64>,
    ) -> Result<(), StorageError>;

    async fn deactivate_watched(&self, watched_id: i64) -> Result<(), StorageError>;

    async fn list_active_watched(&self) -> Result<Vec<WatchedProduct>, StorageError>;

    async fn insert_snapshot(&self, snap: &PriceSnapshot) -> Result<i64, StorageError>;

    async fn latest_snapshot(
        &self,
        watched_id: i64,
    ) -> Result<Option<PriceSnapshot>, StorageError>;

    async fn insert_diff(&self, diff: &Diff) -> Result<i64, StorageError>;

    async fn unnotified_diffs(&self) -> Result<Vec<Diff>, StorageError>;

    async fn mark_notified(&self, diff_ids: &[i64]) -> Result<(), StorageError>;

    // --- catalog cache (for Mini App browse/search) ---
    async fn upsert_catalog_entry(&self, e: &CatalogEntry) -> Result<(), StorageError>;
    async fn search_catalog(&self, q: &str, limit: i64) -> Result<Vec<CatalogEntry>, StorageError>;
    async fn list_catalog_by_reservation(&self, reservation_id: &str) -> Result<Vec<CatalogEntry>, StorageError>;

    // --- price history (for charts) ---
    async fn snapshot_history(
        &self,
        watched_id: i64,
        limit: i64,
    ) -> Result<Vec<HistoryPoint>, StorageError>;

    // --- deck-plan image cache ---
    async fn get_deck_plan(
        &self,
        ship_code: &str,
        deck: i32,
    ) -> Result<Option<DeckPlan>, StorageError>;
    async fn upsert_deck_plan(&self, dp: &DeckPlan) -> Result<(), StorageError>;

    // ============================================================
    // Public (no-login) tier. All additive — the authed methods above
    // are untouched. `account_scope` None is normalized to '' on both
    // write and read; '' is the public promotional series.
    // ============================================================

    // --- sailings ---
    async fn upsert_sailing(
        &self,
        brand: Brand,
        ship_code: &str,
        sail_date: NaiveDate,
    ) -> Result<i64, StorageError>;
    async fn get_sailing(
        &self,
        brand: Brand,
        ship_code: &str,
        sail_date: NaiveDate,
    ) -> Result<Option<Sailing>, StorageError>;
    /// One row per active (sailing, product, scope='') whose sail_date is not
    /// already in the past (1-day grace). Drives the paced public sweep.
    async fn list_sailings_to_scrape(&self) -> Result<Vec<ScrapeTarget>, StorageError>;

    // --- tracked products / shared price series ---
    async fn upsert_tracked_product(
        &self,
        sailing_id: i64,
        product_code: &str,
        category_prefix: &str,
        label: Option<&str>,
        account_scope: Option<&str>,
    ) -> Result<i64, StorageError>;
    /// Record a public-promo observation. ALSO resets consecutive_failures /
    /// last_error and stamps last_success_at.
    async fn record_public_price(
        &self,
        tracked_id: i64,
        snap: &SailingSnapshot,
    ) -> Result<i64, StorageError>;
    /// Most recent stored snapshot — read BEFORE record_public_price so a diff
    /// isn't self-referential.
    async fn latest_sailing_snapshot(
        &self,
        tracked_id: i64,
    ) -> Result<Option<SailingSnapshot>, StorageError>;
    async fn sailing_snapshot_history(
        &self,
        tracked_id: i64,
        limit: i64,
    ) -> Result<Vec<HistoryPoint>, StorageError>;
    /// Increment the failure counter (fixes the silent-freeze bug).
    async fn note_fetch_failure(&self, tracked_id: i64, err: &str) -> Result<(), StorageError>;

    // --- public reads for the lookup UI ---
    async fn list_public_ships(&self) -> Result<Vec<ShipRef>, StorageError>;
    async fn list_public_sailings(
        &self,
        brand: Brand,
        ship_code: &str,
    ) -> Result<Vec<NaiveDate>, StorageError>;
    async fn latest_sailing_prices(
        &self,
        brand: Brand,
        ship_code: &str,
        sail_date: NaiveDate,
    ) -> Result<Vec<PublicPriceDto>, StorageError>;

    // --- channels & subscriptions ---
    async fn upsert_public_channel(&self, ch: &NewPublicChannel<'_>) -> Result<i64, StorageError>;
    async fn subscribe_public(
        &self,
        channel_id: i64,
        tracked_id: i64,
        mode: AlertMode,
        threshold: Option<f64>,
    ) -> Result<i64, StorageError>;
    /// Fan-out targets when a tracked product's price drops.
    async fn list_public_subscriptions_for(
        &self,
        tracked_id: i64,
    ) -> Result<Vec<PublicSubscriber>, StorageError>;
    async fn list_public_subscriptions_for_device(
        &self,
        device_token: &str,
    ) -> Result<Vec<PublicSubscriptionRow>, StorageError>;
    /// Device-scoped deactivate. Returns whether a row actually matched.
    async fn deactivate_public_subscription(
        &self,
        subscription_id: i64,
        device_token: &str,
    ) -> Result<bool, StorageError>;

    // --- sailing diffs ---
    async fn insert_sailing_diff(&self, d: &SailingDiff) -> Result<i64, StorageError>;
    async fn unnotified_sailing_diffs(&self) -> Result<Vec<SailingDiff>, StorageError>;
    async fn mark_sailing_notified(&self, ids: &[i64]) -> Result<(), StorageError>;
}

/// A resolved cruisedeckplans deck-plan image for a ship + deck.
#[derive(Debug, Clone)]
pub struct DeckPlan {
    pub ship_code: String,
    pub deck: i32,
    pub image_url: String,
    pub sourced_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub reservation_id: String,
    pub category_id: String,
    pub category_name: String,
    pub product_code: String,
    pub title: String,
    pub summary: Option<String>,
    pub starting_price: Option<f64>,
    pub currency: Option<String>,
    /// Display-ready promotional price, e.g. `"$107.99"`. Whatever the API gave us.
    pub price_label: Option<String>,
    /// Display-ready base/MSRP price, e.g. `"$135.00"`.
    pub base_price_label: Option<String>,
    /// What the price is "per", e.g. `"Adult Per Day"` / `"Per Seat"`.
    pub unit_label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SubscriberInfo {
    pub user_id: i64,
    pub telegram_chat_id: i64,
    pub telegram_username: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HistoryPoint {
    pub fetched_at: chrono::DateTime<chrono::Utc>,
    pub adult_promo_price: Option<f64>,
}

// ============================================================
// Public-tier DTOs
// ============================================================

/// One unit of work for the paced public-promo sweep.
#[derive(Debug, Clone)]
pub struct ScrapeTarget {
    pub tracked_id: i64,
    pub brand: Brand,
    pub ship_code: String,
    pub sail_date: NaiveDate,
    pub product_code: String,
    pub category_prefix: String,
    /// None == the public promotional series (stored as '').
    pub account_scope: Option<String>,
    pub consecutive_failures: i32,
}

/// A sailing-level (public/promotional) price observation.
#[derive(Debug, Clone)]
pub struct SailingSnapshot {
    pub tracked_id: i64,
    pub fetched_at: DateTime<Utc>,
    /// The fluctuating promotional (sale) price; None when the sale window is
    /// closed (base-only) or the planner isn't open.
    pub adult_promo_price: Option<f64>,
    pub child_promo_price: Option<f64>,
    /// Rack/base price; usually present even when promo is null.
    pub base_price: Option<f64>,
    /// True when a promotional price was present this fetch — distinguishes
    /// "sale ended -> promo null" from "product/planner missing".
    pub promo_present: bool,
    pub raw_response: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct SailingDiff {
    pub id: i64,
    pub tracked_id: i64,
    pub detected_at: DateTime<Utc>,
    pub old_price: f64,
    pub new_price: f64,
    pub delta_pct: f64,
    pub notified: bool,
}

#[derive(Debug, Clone)]
pub struct NewPublicChannel<'a> {
    pub kind: PublicChannelKind,
    /// Web-push endpoint URL, or the email address.
    pub endpoint: &'a str,
    pub p256dh: Option<&'a str>,
    pub auth: Option<&'a str>,
    /// Signed device-cookie id linking this channel to a browser.
    pub device_token: Option<&'a str>,
    /// Email starts unverified; web-push is verified on creation.
    pub verified: bool,
}

/// Fan-out target for a price drop on a sailing series.
#[derive(Debug, Clone)]
pub struct PublicSubscriber {
    pub subscription_id: i64,
    pub channel_id: i64,
    pub kind: PublicChannelKind,
    pub endpoint: String,
    pub p256dh: Option<String>,
    pub auth: Option<String>,
    pub alert_mode: AlertMode,
    pub alert_threshold: Option<f64>,
}

/// A row for the device "manage my alerts" page.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PublicSubscriptionRow {
    pub subscription_id: i64,
    pub tracked_id: i64,
    pub brand: Brand,
    pub ship_code: String,
    pub sail_date: NaiveDate,
    pub product_code: String,
    pub label: Option<String>,
    pub channel_kind: PublicChannelKind,
    pub alert_mode: AlertMode,
    pub alert_threshold: Option<f64>,
    pub latest_promo_price: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ShipRef {
    pub brand: Brand,
    pub ship_code: String,
    pub ship_name: Option<String>,
}

/// A public catalog price row for the read-only lookup UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PublicPriceDto {
    pub product_code: String,
    pub category: String,
    pub title: Option<String>,
    pub promo_price: Option<f64>,
    pub base_price: Option<f64>,
    pub price_label: Option<String>,
    pub base_price_label: Option<String>,
    pub unit_label: Option<String>,
    pub fetched_at: Option<DateTime<Utc>>,
}
