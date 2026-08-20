use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use royaltracker_types::{
    AlertMode, Booking, Brand, Diff, PriceSnapshot, PublicChannelKind, Sailing, User, WatchedProduct,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

use crate::repo::{
    CatalogEntry, DeckPlan, HistoryPoint, NewPublicChannel, NewUser, PriceRepo, PublicPriceDto,
    PublicSubscriber, PublicSubscriptionRow, SailingDiff, SailingSnapshot, ScrapeTarget, ShipRef,
    StorageError, SubscriberInfo,
};

fn scope_to_opt(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn money_label(v: Option<f64>) -> Option<String> {
    v.map(|p| format!("${p:.2}"))
}

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
                                  passenger_id, nights, package_code, stateroom, assigned_stateroom)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(reservation_id) DO UPDATE SET
                brand = excluded.brand,
                account_id = excluded.account_id,
                ship_code = excluded.ship_code,
                sail_date = excluded.sail_date,
                passenger_id = excluded.passenger_id,
                nights = excluded.nights,
                package_code = excluded.package_code,
                stateroom = excluded.stateroom,
                -- Keep a previously-discovered cabin if this refresh couldn't find one
                -- (e.g. transient order-history error), rather than blanking it out.
                assigned_stateroom = COALESCE(excluded.assigned_stateroom, bookings.assigned_stateroom),
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
        .bind(&b.stateroom)
        .bind(&b.assigned_stateroom)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_bookings(&self) -> Result<Vec<Booking>, StorageError> {
        let rows = sqlx::query(
            "SELECT reservation_id, brand, account_id, ship_code, sail_date, passenger_id, nights, package_code, stateroom, assigned_stateroom FROM bookings ORDER BY sail_date",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_booking_sqlite).collect()
    }

    async fn list_bookings_for_user(&self, user_id: i64) -> Result<Vec<Booking>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT b.reservation_id, b.brand, b.account_id, b.ship_code, b.sail_date,
                      b.passenger_id, b.nights, b.package_code, b.stateroom, b.assigned_stateroom
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

    async fn list_subscribers_for_reservation(
        &self,
        reservation_id: &str,
    ) -> Result<Vec<SubscriberInfo>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT u.id AS user_id, u.telegram_chat_id, u.telegram_username
               FROM booking_subscribers s
               JOIN users u ON u.id = s.user_id
               WHERE s.reservation_id = ?1 AND u.active = 1"#,
        )
        .bind(reservation_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| -> Result<SubscriberInfo, StorageError> {
                Ok(SubscriberInfo {
                    user_id: r.try_get("user_id")?,
                    telegram_chat_id: r.try_get("telegram_chat_id")?,
                    telegram_username: r.try_get("telegram_username")?,
                })
            })
            .collect()
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
            ON CONFLICT(reservation_id, product_code) DO UPDATE SET
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

    async fn get_deck_plan(
        &self,
        ship_code: &str,
        deck: i32,
    ) -> Result<Option<DeckPlan>, StorageError> {
        let row = sqlx::query(
            "SELECT ship_code, deck, image_url, sourced_at FROM deck_plans WHERE ship_code = ?1 AND deck = ?2",
        )
        .bind(ship_code)
        .bind(deck)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| {
            let s: String = r.try_get("sourced_at")?;
            Ok::<_, StorageError>(DeckPlan {
                ship_code: r.try_get("ship_code")?,
                deck: r.try_get("deck")?,
                image_url: r.try_get("image_url")?,
                sourced_at: DateTime::parse_from_rfc3339(&s)
                    .map_err(|e| sqlx::Error::Decode(Box::new(e)))?
                    .with_timezone(&Utc),
            })
        })
        .transpose()
    }

    async fn upsert_deck_plan(&self, dp: &DeckPlan) -> Result<(), StorageError> {
        sqlx::query(
            r#"INSERT INTO deck_plans (ship_code, deck, image_url, sourced_at)
               VALUES (?1, ?2, ?3, ?4)
               ON CONFLICT(ship_code, deck) DO UPDATE SET
                   image_url = excluded.image_url,
                   sourced_at = excluded.sourced_at"#,
        )
        .bind(&dp.ship_code)
        .bind(dp.deck)
        .bind(&dp.image_url)
        .bind(dp.sourced_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ============================================================
    // Public tier
    // ============================================================

    async fn upsert_sailing(
        &self,
        brand: Brand,
        ship_code: &str,
        sail_date: NaiveDate,
    ) -> Result<i64, StorageError> {
        let row = sqlx::query(
            r#"INSERT INTO sailings (brand, ship_code, sail_date, active)
               VALUES (?1, ?2, ?3, 1)
               ON CONFLICT(brand, ship_code, sail_date) DO UPDATE SET active = 1
               RETURNING id"#,
        )
        .bind(brand.as_str())
        .bind(ship_code)
        .bind(sail_date.format("%Y-%m-%d").to_string())
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("id")?)
    }

    async fn get_sailing(
        &self,
        brand: Brand,
        ship_code: &str,
        sail_date: NaiveDate,
    ) -> Result<Option<Sailing>, StorageError> {
        let row = sqlx::query(
            "SELECT id, brand, ship_code, sail_date FROM sailings WHERE brand = ?1 AND ship_code = ?2 AND sail_date = ?3",
        )
        .bind(brand.as_str())
        .bind(ship_code)
        .bind(sail_date.format("%Y-%m-%d").to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_sailing_sqlite).transpose()
    }

    async fn list_sailings_to_scrape(&self) -> Result<Vec<ScrapeTarget>, StorageError> {
        let cutoff = (Utc::now().date_naive() - chrono::Days::new(1))
            .format("%Y-%m-%d")
            .to_string();
        let rows = sqlx::query(
            r#"SELECT t.id AS tracked_id, s.brand, s.ship_code, s.sail_date,
                      t.product_code, t.category_prefix, t.account_scope, t.consecutive_failures
               FROM tracked_products t
               JOIN sailings s ON s.id = t.sailing_id
               WHERE t.active = 1 AND s.active = 1 AND t.account_scope = ''
                 AND s.sail_date >= ?1
               ORDER BY s.brand, s.ship_code, s.sail_date"#,
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_scrape_target_sqlite).collect()
    }

    async fn upsert_tracked_product(
        &self,
        sailing_id: i64,
        product_code: &str,
        category_prefix: &str,
        label: Option<&str>,
        account_scope: Option<&str>,
    ) -> Result<i64, StorageError> {
        let row = sqlx::query(
            r#"INSERT INTO tracked_products (sailing_id, product_code, category_prefix, label, account_scope, active)
               VALUES (?1, ?2, ?3, ?4, ?5, 1)
               ON CONFLICT(sailing_id, product_code, account_scope) DO UPDATE SET
                   label = COALESCE(excluded.label, tracked_products.label),
                   category_prefix = excluded.category_prefix,
                   active = 1
               RETURNING id"#,
        )
        .bind(sailing_id)
        .bind(product_code)
        .bind(category_prefix)
        .bind(label)
        .bind(account_scope.unwrap_or(""))
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("id")?)
    }

    async fn record_public_price(
        &self,
        tracked_id: i64,
        snap: &SailingSnapshot,
    ) -> Result<i64, StorageError> {
        let raw = serde_json::to_string(&snap.raw_response)?;
        let row = sqlx::query(
            r#"INSERT INTO sailing_price_snapshots
                   (tracked_id, fetched_at, adult_promo_price, child_promo_price, base_price, promo_present, raw_response)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
               ON CONFLICT(tracked_id, fetched_at) DO UPDATE SET
                   adult_promo_price = excluded.adult_promo_price,
                   child_promo_price = excluded.child_promo_price,
                   base_price = excluded.base_price,
                   promo_present = excluded.promo_present,
                   raw_response = excluded.raw_response
               RETURNING id"#,
        )
        .bind(tracked_id)
        .bind(snap.fetched_at.to_rfc3339())
        .bind(snap.adult_promo_price)
        .bind(snap.child_promo_price)
        .bind(snap.base_price)
        .bind(snap.promo_present as i64)
        .bind(raw)
        .fetch_one(&self.pool)
        .await?;
        sqlx::query(
            "UPDATE tracked_products SET consecutive_failures = 0, last_error = NULL, last_fetch_at = datetime('now'), last_success_at = datetime('now') WHERE id = ?1",
        )
        .bind(tracked_id)
        .execute(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("id")?)
    }

    async fn latest_sailing_snapshot(
        &self,
        tracked_id: i64,
    ) -> Result<Option<SailingSnapshot>, StorageError> {
        let row = sqlx::query(
            r#"SELECT tracked_id, fetched_at, adult_promo_price, child_promo_price, base_price, promo_present, raw_response
               FROM sailing_price_snapshots
               WHERE tracked_id = ?1
               ORDER BY fetched_at DESC
               LIMIT 1"#,
        )
        .bind(tracked_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(row_to_sailing_snapshot_sqlite).transpose()
    }

    async fn sailing_snapshot_history(
        &self,
        tracked_id: i64,
        limit: i64,
    ) -> Result<Vec<HistoryPoint>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT fetched_at, adult_promo_price FROM sailing_price_snapshots
               WHERE tracked_id = ?1 ORDER BY fetched_at DESC LIMIT ?2"#,
        )
        .bind(tracked_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| -> Result<HistoryPoint, StorageError> {
                let s: String = r.try_get("fetched_at")?;
                Ok(HistoryPoint {
                    fetched_at: DateTime::parse_from_rfc3339(&s)
                        .map_err(|e| sqlx::Error::Decode(Box::new(e)))?
                        .with_timezone(&Utc),
                    adult_promo_price: r.try_get("adult_promo_price")?,
                })
            })
            .collect()
    }

    async fn note_fetch_failure(&self, tracked_id: i64, err: &str) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE tracked_products SET consecutive_failures = consecutive_failures + 1, last_error = ?1, last_fetch_at = datetime('now') WHERE id = ?2",
        )
        .bind(err)
        .bind(tracked_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_public_ships(&self) -> Result<Vec<ShipRef>, StorageError> {
        let rows = sqlx::query(
            "SELECT DISTINCT brand, ship_code FROM sailings WHERE active = 1 ORDER BY ship_code",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_shipref_sqlite).collect()
    }

    async fn list_public_sailings(
        &self,
        brand: Brand,
        ship_code: &str,
    ) -> Result<Vec<NaiveDate>, StorageError> {
        let rows = sqlx::query(
            "SELECT sail_date FROM sailings WHERE brand = ?1 AND ship_code = ?2 AND active = 1 ORDER BY sail_date",
        )
        .bind(brand.as_str())
        .bind(ship_code)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| -> Result<NaiveDate, StorageError> {
                let s: String = r.try_get("sail_date")?;
                NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .map_err(|e| StorageError::from(sqlx::Error::Decode(Box::new(e))))
            })
            .collect()
    }

    async fn latest_sailing_prices(
        &self,
        brand: Brand,
        ship_code: &str,
        sail_date: NaiveDate,
    ) -> Result<Vec<PublicPriceDto>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT t.product_code, t.category_prefix, t.label,
                      sp.adult_promo_price, sp.base_price, sp.fetched_at
               FROM tracked_products t
               JOIN sailings s ON s.id = t.sailing_id
               LEFT JOIN sailing_price_snapshots sp ON sp.id = (
                   SELECT id FROM sailing_price_snapshots
                   WHERE tracked_id = t.id ORDER BY fetched_at DESC LIMIT 1)
               WHERE s.brand = ?1 AND s.ship_code = ?2 AND s.sail_date = ?3
                 AND t.account_scope = '' AND t.active = 1
               ORDER BY t.category_prefix, t.label"#,
        )
        .bind(brand.as_str())
        .bind(ship_code)
        .bind(sail_date.format("%Y-%m-%d").to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_public_price_sqlite).collect()
    }

    async fn upsert_public_channel(&self, ch: &NewPublicChannel<'_>) -> Result<i64, StorageError> {
        let row = sqlx::query(
            r#"INSERT INTO public_channels (kind, endpoint, p256dh, auth, device_token, verified)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6)
               ON CONFLICT(kind, endpoint) DO UPDATE SET
                   p256dh = excluded.p256dh,
                   auth = excluded.auth,
                   device_token = COALESCE(excluded.device_token, public_channels.device_token),
                   verified = (public_channels.verified OR excluded.verified),
                   last_seen_at = datetime('now')
               RETURNING id"#,
        )
        .bind(ch.kind.as_str())
        .bind(ch.endpoint)
        .bind(ch.p256dh)
        .bind(ch.auth)
        .bind(ch.device_token)
        .bind(ch.verified as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("id")?)
    }

    async fn subscribe_public(
        &self,
        channel_id: i64,
        tracked_id: i64,
        mode: AlertMode,
        threshold: Option<f64>,
    ) -> Result<i64, StorageError> {
        let row = sqlx::query(
            r#"INSERT INTO public_subscriptions (channel_id, tracked_id, alert_mode, alert_threshold, active)
               VALUES (?1, ?2, ?3, ?4, 1)
               ON CONFLICT(channel_id, tracked_id) DO UPDATE SET
                   alert_mode = excluded.alert_mode,
                   alert_threshold = excluded.alert_threshold,
                   active = 1
               RETURNING id"#,
        )
        .bind(channel_id)
        .bind(tracked_id)
        .bind(mode.as_str())
        .bind(threshold)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("id")?)
    }

    async fn list_public_subscriptions_for(
        &self,
        tracked_id: i64,
    ) -> Result<Vec<PublicSubscriber>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT ps.id AS subscription_id, c.id AS channel_id, c.kind, c.endpoint,
                      c.p256dh, c.auth, ps.alert_mode, ps.alert_threshold
               FROM public_subscriptions ps
               JOIN public_channels c ON c.id = ps.channel_id
               WHERE ps.tracked_id = ?1 AND ps.active = 1"#,
        )
        .bind(tracked_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_public_subscriber_sqlite).collect()
    }

    async fn list_public_subscriptions_for_device(
        &self,
        device_token: &str,
    ) -> Result<Vec<PublicSubscriptionRow>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT ps.id AS subscription_id, ps.tracked_id, s.brand, s.ship_code, s.sail_date,
                      t.product_code, t.label, c.kind AS channel_kind, ps.alert_mode, ps.alert_threshold,
                      (SELECT adult_promo_price FROM sailing_price_snapshots
                       WHERE tracked_id = t.id ORDER BY fetched_at DESC LIMIT 1) AS latest_promo_price
               FROM public_subscriptions ps
               JOIN public_channels c ON c.id = ps.channel_id
               JOIN tracked_products t ON t.id = ps.tracked_id
               JOIN sailings s ON s.id = t.sailing_id
               WHERE c.device_token = ?1 AND ps.active = 1
               ORDER BY s.sail_date"#,
        )
        .bind(device_token)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_public_sub_row_sqlite).collect()
    }

    async fn deactivate_public_subscription(
        &self,
        subscription_id: i64,
        device_token: &str,
    ) -> Result<bool, StorageError> {
        let res = sqlx::query(
            r#"UPDATE public_subscriptions SET active = 0
               WHERE id = ?1 AND channel_id IN (
                   SELECT id FROM public_channels WHERE device_token = ?2)"#,
        )
        .bind(subscription_id)
        .bind(device_token)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    async fn deactivate_channel_by_endpoint(&self, endpoint: &str) -> Result<(), StorageError> {
        sqlx::query(
            r#"UPDATE public_subscriptions SET active = 0
               WHERE channel_id IN (SELECT id FROM public_channels WHERE endpoint = ?1)"#,
        )
        .bind(endpoint)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn insert_sailing_diff(&self, d: &SailingDiff) -> Result<i64, StorageError> {
        let row = sqlx::query(
            r#"INSERT INTO sailing_diffs (tracked_id, detected_at, old_price, new_price, delta_pct, notified)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id"#,
        )
        .bind(d.tracked_id)
        .bind(d.detected_at.to_rfc3339())
        .bind(d.old_price)
        .bind(d.new_price)
        .bind(d.delta_pct)
        .bind(d.notified as i64)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("id")?)
    }

    async fn unnotified_sailing_diffs(&self) -> Result<Vec<SailingDiff>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT id, tracked_id, detected_at, old_price, new_price, delta_pct, notified
               FROM sailing_diffs WHERE notified = 0 ORDER BY detected_at"#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(row_to_sailing_diff_sqlite).collect()
    }

    async fn mark_sailing_notified(&self, ids: &[i64]) -> Result<(), StorageError> {
        if ids.is_empty() {
            return Ok(());
        }
        let placeholders = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
        let sql = format!("UPDATE sailing_diffs SET notified = 1 WHERE id IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        for id in ids {
            q = q.bind(id);
        }
        q.execute(&self.pool).await?;
        Ok(())
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
        stateroom: r.try_get("stateroom")?,
        assigned_stateroom: r.try_get("assigned_stateroom")?,
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

fn parse_brand_sqlite(s: &str) -> Result<Brand, StorageError> {
    Brand::from_str(s).map_err(|e| StorageError::from(sqlx::Error::Decode(Box::new(e))))
}

fn parse_ts_sqlite(s: &str) -> Result<DateTime<Utc>, StorageError> {
    Ok(DateTime::parse_from_rfc3339(s)
        .map_err(|e| sqlx::Error::Decode(Box::new(e)))?
        .with_timezone(&Utc))
}

fn row_to_sailing_sqlite(r: sqlx::sqlite::SqliteRow) -> Result<Sailing, StorageError> {
    let brand_s: String = r.try_get("brand")?;
    let sail_s: String = r.try_get("sail_date")?;
    Ok(Sailing {
        id: r.try_get("id")?,
        brand: parse_brand_sqlite(&brand_s)?,
        ship_code: r.try_get("ship_code")?,
        sail_date: NaiveDate::parse_from_str(&sail_s, "%Y-%m-%d")
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
    })
}

fn row_to_scrape_target_sqlite(r: sqlx::sqlite::SqliteRow) -> Result<ScrapeTarget, StorageError> {
    let brand_s: String = r.try_get("brand")?;
    let sail_s: String = r.try_get("sail_date")?;
    let scope: String = r.try_get("account_scope")?;
    Ok(ScrapeTarget {
        tracked_id: r.try_get("tracked_id")?,
        brand: parse_brand_sqlite(&brand_s)?,
        ship_code: r.try_get("ship_code")?,
        sail_date: NaiveDate::parse_from_str(&sail_s, "%Y-%m-%d")
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        product_code: r.try_get("product_code")?,
        category_prefix: r.try_get("category_prefix")?,
        account_scope: scope_to_opt(scope),
        consecutive_failures: r.try_get("consecutive_failures")?,
    })
}

fn row_to_sailing_snapshot_sqlite(
    r: sqlx::sqlite::SqliteRow,
) -> Result<SailingSnapshot, StorageError> {
    let fetched_s: String = r.try_get("fetched_at")?;
    let raw_s: String = r.try_get("raw_response")?;
    let promo_i: i64 = r.try_get("promo_present")?;
    Ok(SailingSnapshot {
        tracked_id: r.try_get("tracked_id")?,
        fetched_at: parse_ts_sqlite(&fetched_s)?,
        adult_promo_price: r.try_get("adult_promo_price")?,
        child_promo_price: r.try_get("child_promo_price")?,
        base_price: r.try_get("base_price")?,
        promo_present: promo_i != 0,
        raw_response: serde_json::from_str(&raw_s)?,
    })
}

fn row_to_shipref_sqlite(r: sqlx::sqlite::SqliteRow) -> Result<ShipRef, StorageError> {
    let brand_s: String = r.try_get("brand")?;
    let ship_code: String = r.try_get("ship_code")?;
    let ship_name = royaltracker_types::ship_name(&ship_code).map(|s| s.to_string());
    Ok(ShipRef {
        brand: parse_brand_sqlite(&brand_s)?,
        ship_code,
        ship_name,
    })
}

fn row_to_public_price_sqlite(r: sqlx::sqlite::SqliteRow) -> Result<PublicPriceDto, StorageError> {
    let promo: Option<f64> = r.try_get("adult_promo_price")?;
    let base: Option<f64> = r.try_get("base_price")?;
    let fetched_s: Option<String> = r.try_get("fetched_at")?;
    let fetched_at = match fetched_s {
        Some(s) => Some(parse_ts_sqlite(&s)?),
        None => None,
    };
    Ok(PublicPriceDto {
        product_code: r.try_get("product_code")?,
        category: r.try_get("category_prefix")?,
        title: r.try_get("label")?,
        promo_price: promo,
        base_price: base,
        price_label: money_label(promo),
        base_price_label: money_label(base),
        unit_label: None,
        fetched_at,
    })
}

fn row_to_public_subscriber_sqlite(
    r: sqlx::sqlite::SqliteRow,
) -> Result<PublicSubscriber, StorageError> {
    let kind_s: String = r.try_get("kind")?;
    let mode_s: String = r.try_get("alert_mode")?;
    Ok(PublicSubscriber {
        subscription_id: r.try_get("subscription_id")?,
        channel_id: r.try_get("channel_id")?,
        kind: PublicChannelKind::from_str(&kind_s)
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        endpoint: r.try_get("endpoint")?,
        p256dh: r.try_get("p256dh")?,
        auth: r.try_get("auth")?,
        alert_mode: AlertMode::from_str(&mode_s).map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        alert_threshold: r.try_get("alert_threshold")?,
    })
}

fn row_to_public_sub_row_sqlite(
    r: sqlx::sqlite::SqliteRow,
) -> Result<PublicSubscriptionRow, StorageError> {
    let brand_s: String = r.try_get("brand")?;
    let sail_s: String = r.try_get("sail_date")?;
    let kind_s: String = r.try_get("channel_kind")?;
    let mode_s: String = r.try_get("alert_mode")?;
    Ok(PublicSubscriptionRow {
        subscription_id: r.try_get("subscription_id")?,
        tracked_id: r.try_get("tracked_id")?,
        brand: parse_brand_sqlite(&brand_s)?,
        ship_code: r.try_get("ship_code")?,
        sail_date: NaiveDate::parse_from_str(&sail_s, "%Y-%m-%d")
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        product_code: r.try_get("product_code")?,
        label: r.try_get("label")?,
        channel_kind: PublicChannelKind::from_str(&kind_s)
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        alert_mode: AlertMode::from_str(&mode_s).map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        alert_threshold: r.try_get("alert_threshold")?,
        latest_promo_price: r.try_get("latest_promo_price")?,
    })
}

fn row_to_sailing_diff_sqlite(r: sqlx::sqlite::SqliteRow) -> Result<SailingDiff, StorageError> {
    let detected_s: String = r.try_get("detected_at")?;
    let notified_i: i64 = r.try_get("notified")?;
    Ok(SailingDiff {
        id: r.try_get("id")?,
        tracked_id: r.try_get("tracked_id")?,
        detected_at: parse_ts_sqlite(&detected_s)?,
        old_price: r.try_get("old_price")?,
        new_price: r.try_get("new_price")?,
        delta_pct: r.try_get("delta_pct")?,
        notified: notified_i != 0,
    })
}
