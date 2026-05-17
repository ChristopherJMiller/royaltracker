use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use royaltracker_types::{AlertMode, Booking, Brand, Diff, PriceSnapshot, User, WatchedProduct};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

use crate::repo::{CatalogEntry, HistoryPoint, NewUser, PriceRepo, StorageError};

#[derive(Clone)]
pub struct SqliteRepo {
    pool: SqlitePool,
}

impl SqliteRepo {
    pub async fn connect(url: &str) -> Result<Self, StorageError> {
        let opts = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .busy_timeout(std::time::Duration::from_secs(5))
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;

        Ok(Self { pool })
    }
}

fn row_to_user_sqlite(r: sqlx::sqlite::SqliteRow) -> Result<User, StorageError> {
    let brand_s: String = r.try_get("brand_pref")?;
    let active_i: i64 = r.try_get("active")?;
    Ok(User {
        id: r.try_get("id")?,
        telegram_chat_id: r.try_get("telegram_chat_id")?,
        telegram_username: r.try_get("telegram_username")?,
        rcg_username: r.try_get("rcg_username")?,
        rcg_password_ct: r.try_get("rcg_password_ct")?,
        rcg_password_nonce: r.try_get("rcg_password_nonce")?,
        brand_pref: Brand::from_str(&brand_s).map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        active: active_i != 0,
    })
}

#[async_trait]
impl PriceRepo for SqliteRepo {
    async fn migrate(&self) -> Result<(), StorageError> {
        sqlx::migrate!("../../migrations/sqlite")
            .run(&self.pool)
            .await?;
        Ok(())
    }

    async fn upsert_user(&self, u: &NewUser<'_>) -> Result<i64, StorageError> {
        let row = sqlx::query(
            r#"
            INSERT INTO users (telegram_chat_id, telegram_username, rcg_username,
                               rcg_password_ct, rcg_password_nonce, brand_pref, active)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)
            ON CONFLICT(telegram_chat_id) DO UPDATE SET
                telegram_username = excluded.telegram_username,
                rcg_username = excluded.rcg_username,
                rcg_password_ct = excluded.rcg_password_ct,
                rcg_password_nonce = excluded.rcg_password_nonce,
                brand_pref = excluded.brand_pref,
                active = 1,
                updated_at = datetime('now')
            RETURNING id
            "#,
        )
        .bind(u.telegram_chat_id)
        .bind(u.telegram_username)
        .bind(u.rcg_username)
        .bind(u.rcg_password_ct)
        .bind(u.rcg_password_nonce)
        .bind(u.brand_pref.as_str())
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("id")?)
    }

    async fn get_user_by_chat_id(&self, chat_id: i64) -> Result<Option<User>, StorageError> {
        let row = sqlx::query(
            r#"SELECT id, telegram_chat_id, telegram_username, rcg_username,
                      rcg_password_ct, rcg_password_nonce, brand_pref, active
               FROM users WHERE telegram_chat_id = ?1"#,
        )
        .bind(chat_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(row_to_user_sqlite).transpose()
    }

    async fn list_active_users(&self) -> Result<Vec<User>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT id, telegram_chat_id, telegram_username, rcg_username,
                      rcg_password_ct, rcg_password_nonce, brand_pref, active
               FROM users WHERE active = 1"#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_user_sqlite).collect()
    }

    async fn deactivate_user(&self, chat_id: i64) -> Result<(), StorageError> {
        sqlx::query("UPDATE users SET active = 0, updated_at = datetime('now') WHERE telegram_chat_id = ?1")
            .bind(chat_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_user_brand(&self, chat_id: i64, brand: Brand) -> Result<(), StorageError> {
        sqlx::query("UPDATE users SET brand_pref = ?1, updated_at = datetime('now') WHERE telegram_chat_id = ?2")
            .bind(brand.as_str())
            .bind(chat_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn upsert_booking(&self, b: &Booking) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            INSERT INTO bookings (reservation_id, brand, account_id, ship_code, sail_date,
                                  passenger_id, nights, package_code)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(reservation_id) DO UPDATE SET
                brand = excluded.brand,
                account_id = excluded.account_id,
                ship_code = excluded.ship_code,
                sail_date = excluded.sail_date,
                passenger_id = excluded.passenger_id,
                nights = excluded.nights,
                package_code = excluded.package_code,
                updated_at = datetime('now')
            "#,
        )
        .bind(&b.reservation_id)
        .bind(b.brand.as_str())
        .bind(&b.account_id)
        .bind(&b.ship_code)
        .bind(b.sail_date.format("%Y-%m-%d").to_string())
        .bind(&b.passenger_id)
        .bind(b.nights)
        .bind(&b.package_code)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_bookings(&self) -> Result<Vec<Booking>, StorageError> {
        let rows = sqlx::query(
            "SELECT reservation_id, brand, account_id, ship_code, sail_date, passenger_id, nights, package_code FROM bookings ORDER BY sail_date",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_booking_sqlite).collect()
    }

    async fn list_bookings_for_user(&self, user_id: i64) -> Result<Vec<Booking>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT b.reservation_id, b.brand, b.account_id, b.ship_code, b.sail_date,
                      b.passenger_id, b.nights, b.package_code
               FROM bookings b
               JOIN booking_subscribers s ON s.reservation_id = b.reservation_id
               WHERE s.user_id = ?1
               ORDER BY b.sail_date"#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_booking_sqlite).collect()
    }

    async fn subscribe_user_to_booking(
        &self,
        reservation_id: &str,
        user_id: i64,
    ) -> Result<(), StorageError> {
        sqlx::query(
            r#"INSERT INTO booking_subscribers (reservation_id, user_id)
               VALUES (?1, ?2)
               ON CONFLICT(reservation_id, user_id) DO NOTHING"#,
        )
        .bind(reservation_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn user_owns_reservation(
        &self,
        user_id: i64,
        reservation_id: &str,
    ) -> Result<bool, StorageError> {
        let row = sqlx::query(
            "SELECT 1 AS hit FROM booking_subscribers WHERE user_id = ?1 AND reservation_id = ?2",
        )
        .bind(user_id)
        .bind(reservation_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.is_some())
    }

    async fn upsert_watched(
        &self,
        reservation_id: &str,
        category_prefix: &str,
        product_code: &str,
        label: Option<&str>,
    ) -> Result<i64, StorageError> {
        let row = sqlx::query(
            r#"
            INSERT INTO products_watched (reservation_id, category_prefix, product_code, label, active)
            VALUES (?1, ?2, ?3, ?4, 1)
            ON CONFLICT(reservation_id, category_prefix, product_code) DO UPDATE SET
                label = COALESCE(excluded.label, products_watched.label),
                active = 1
            RETURNING id
            "#,
        )
        .bind(reservation_id)
        .bind(category_prefix)
        .bind(product_code)
        .bind(label)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("id")?)
    }

    async fn list_active_watched(&self) -> Result<Vec<WatchedProduct>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, reservation_id, category_prefix, product_code, label, active, alert_mode, alert_threshold FROM products_watched WHERE active = 1",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_watched_sqlite).collect()
    }

    async fn set_watch_alert(
        &self,
        watched_id: i64,
        mode: AlertMode,
        threshold: Option<f64>,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE products_watched SET alert_mode = ?1, alert_threshold = ?2 WHERE id = ?3",
        )
        .bind(mode.as_str())
        .bind(threshold)
        .bind(watched_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn deactivate_watched(&self, watched_id: i64) -> Result<(), StorageError> {
        sqlx::query("UPDATE products_watched SET active = 0 WHERE id = ?1")
            .bind(watched_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn insert_snapshot(&self, s: &PriceSnapshot) -> Result<i64, StorageError> {
        let raw = serde_json::to_string(&s.raw_response)?;
        let row = sqlx::query(
            r#"
            INSERT INTO price_snapshots (watched_id, fetched_at, adult_promo_price, child_promo_price, raw_response)
            VALUES (?1, ?2, ?3, ?4, ?5)
            RETURNING id
            "#,
        )
        .bind(s.watched_id)
        .bind(s.fetched_at.to_rfc3339())
        .bind(s.adult_promo_price)
        .bind(s.child_promo_price)
        .bind(raw)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("id")?)
    }

    async fn latest_snapshot(
        &self,
        watched_id: i64,
    ) -> Result<Option<PriceSnapshot>, StorageError> {
        let row = sqlx::query(
            r#"
            SELECT watched_id, fetched_at, adult_promo_price, child_promo_price, raw_response
            FROM price_snapshots
            WHERE watched_id = ?1
            ORDER BY fetched_at DESC
            LIMIT 1
            "#,
        )
        .bind(watched_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| {
            let fetched_s: String = r.try_get("fetched_at")?;
            let raw_s: String = r.try_get("raw_response")?;
            Ok::<_, StorageError>(PriceSnapshot {
                watched_id: r.try_get("watched_id")?,
                fetched_at: DateTime::parse_from_rfc3339(&fetched_s)
                    .map_err(|e| sqlx::Error::Decode(Box::new(e)))?
                    .with_timezone(&Utc),
                adult_promo_price: r.try_get("adult_promo_price")?,
                child_promo_price: r.try_get("child_promo_price")?,
                raw_response: serde_json::from_str(&raw_s)?,
            })
        })
        .transpose()
    }

    async fn insert_diff(&self, d: &Diff) -> Result<i64, StorageError> {
        let row = sqlx::query(
            r#"
            INSERT INTO diffs (watched_id, detected_at, old_price, new_price, delta_pct, notified)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            RETURNING id
            "#,
        )
        .bind(d.watched_id)
        .bind(d.detected_at.to_rfc3339())
        .bind(d.old_price)
        .bind(d.new_price)
        .bind(d.delta_pct)
        .bind(d.notified as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("id")?)
    }

    async fn unnotified_diffs(&self) -> Result<Vec<Diff>, StorageError> {
        let rows = sqlx::query(
            r#"
            SELECT id, watched_id, detected_at, old_price, new_price, delta_pct, notified
            FROM diffs
            WHERE notified = 0
            ORDER BY detected_at
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| -> Result<Diff, StorageError> {
                let detected_s: String = r.try_get("detected_at")?;
                let notified_i: i64 = r.try_get("notified")?;
                Ok(Diff {
                    watched_id: r.try_get("watched_id")?,
                    detected_at: DateTime::parse_from_rfc3339(&detected_s)
                        .map_err(|e| sqlx::Error::Decode(Box::new(e)))?
                        .with_timezone(&Utc),
                    old_price: r.try_get("old_price")?,
                    new_price: r.try_get("new_price")?,
                    delta_pct: r.try_get("delta_pct")?,
                    notified: notified_i != 0,
                })
            })
            .collect()
    }

    async fn mark_notified(&self, ids: &[i64]) -> Result<(), StorageError> {
        if ids.is_empty() {
            return Ok(());
        }
        let placeholders = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
        let sql = format!("UPDATE diffs SET notified = 1 WHERE id IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        for id in ids {
            q = q.bind(id);
        }
        q.execute(&self.pool).await?;
        Ok(())
    }

    async fn upsert_catalog_entry(&self, e: &CatalogEntry) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            INSERT INTO catalog_cache (reservation_id, category_id, category_name,
                                       product_code, title, summary, starting_price, currency,
                                       price_label, base_price_label, unit_label)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(reservation_id, category_id, product_code) DO UPDATE SET
                category_name    = excluded.category_name,
                title            = excluded.title,
                summary          = excluded.summary,
                starting_price   = excluded.starting_price,
                currency         = excluded.currency,
                price_label      = excluded.price_label,
                base_price_label = excluded.base_price_label,
                unit_label       = excluded.unit_label,
                fetched_at       = datetime('now')
            "#,
        )
        .bind(&e.reservation_id)
        .bind(&e.category_id)
        .bind(&e.category_name)
        .bind(&e.product_code)
        .bind(&e.title)
        .bind(&e.summary)
        .bind(e.starting_price)
        .bind(&e.currency)
        .bind(&e.price_label)
        .bind(&e.base_price_label)
        .bind(&e.unit_label)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn search_catalog(&self, q: &str, limit: i64) -> Result<Vec<CatalogEntry>, StorageError> {
        let pattern = format!("%{}%", q.replace('%', "\\%"));
        let rows = sqlx::query(
            r#"SELECT reservation_id, category_id, category_name, product_code,
                      title, summary, starting_price, currency,
                      price_label, base_price_label, unit_label
               FROM catalog_cache
               WHERE title LIKE ?1 COLLATE NOCASE
               ORDER BY title
               LIMIT ?2"#,
        )
        .bind(pattern)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_catalog).collect()
    }

    async fn list_catalog_by_reservation(
        &self,
        reservation_id: &str,
    ) -> Result<Vec<CatalogEntry>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT reservation_id, category_id, category_name, product_code,
                      title, summary, starting_price, currency,
                      price_label, base_price_label, unit_label
               FROM catalog_cache
               WHERE reservation_id = ?1
               ORDER BY category_name, title"#,
        )
        .bind(reservation_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_catalog).collect()
    }

    async fn snapshot_history(
        &self,
        watched_id: i64,
        limit: i64,
    ) -> Result<Vec<HistoryPoint>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT fetched_at, adult_promo_price
               FROM price_snapshots
               WHERE watched_id = ?1
               ORDER BY fetched_at DESC
               LIMIT ?2"#,
        )
        .bind(watched_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| -> Result<HistoryPoint, StorageError> {
                let s: String = r.try_get("fetched_at")?;
                let fetched_at = DateTime::parse_from_rfc3339(&s)
                    .map_err(|e| sqlx::Error::Decode(Box::new(e)))?
                    .with_timezone(&Utc);
                Ok(HistoryPoint {
                    fetched_at,
                    adult_promo_price: r.try_get("adult_promo_price")?,
                })
            })
            .collect()
    }
}

fn row_to_booking_sqlite(r: sqlx::sqlite::SqliteRow) -> Result<Booking, StorageError> {
    let brand_s: String = r.try_get("brand")?;
    let sail_s: String = r.try_get("sail_date")?;
    Ok(Booking {
        reservation_id: r.try_get("reservation_id")?,
        brand: Brand::from_str(&brand_s).map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        account_id: r.try_get("account_id")?,
        ship_code: r.try_get("ship_code")?,
        sail_date: NaiveDate::parse_from_str(&sail_s, "%Y-%m-%d")
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        passenger_id: r.try_get("passenger_id")?,
        nights: r.try_get("nights")?,
        package_code: r.try_get("package_code")?,
    })
}

fn row_to_watched_sqlite(r: sqlx::sqlite::SqliteRow) -> Result<WatchedProduct, StorageError> {
    let active_i: i64 = r.try_get("active")?;
    let mode_s: String = r.try_get("alert_mode")?;
    Ok(WatchedProduct {
        id: r.try_get("id")?,
        reservation_id: r.try_get("reservation_id")?,
        category_prefix: r.try_get("category_prefix")?,
        product_code: r.try_get("product_code")?,
        label: r.try_get("label")?,
        active: active_i != 0,
        alert_mode: AlertMode::from_str(&mode_s)
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        alert_threshold: r.try_get("alert_threshold")?,
    })
}

fn row_to_catalog(r: sqlx::sqlite::SqliteRow) -> Result<CatalogEntry, StorageError> {
    Ok(CatalogEntry {
        reservation_id: r.try_get("reservation_id")?,
        category_id: r.try_get("category_id")?,
        category_name: r.try_get("category_name")?,
        product_code: r.try_get("product_code")?,
        title: r.try_get("title")?,
        summary: r.try_get("summary")?,
        starting_price: r.try_get("starting_price")?,
        currency: r.try_get("currency")?,
        price_label: r.try_get("price_label")?,
        base_price_label: r.try_get("base_price_label")?,
        unit_label: r.try_get("unit_label")?,
    })
}
