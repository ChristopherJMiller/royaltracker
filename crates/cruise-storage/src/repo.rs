use async_trait::async_trait;
use cruise_types::{Booking, Brand, Diff, PriceSnapshot, User, WatchedProduct};

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

    async fn upsert_watched(
        &self,
        reservation_id: &str,
        category_prefix: &str,
        product_code: &str,
        label: Option<&str>,
    ) -> Result<i64, StorageError>;

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

#[derive(Debug, Clone, serde::Serialize)]
pub struct HistoryPoint {
    pub fetched_at: chrono::DateTime<chrono::Utc>,
    pub adult_promo_price: Option<f64>,
}
