use async_trait::async_trait;
use cruise_types::{Booking, Brand, Diff, PriceSnapshot, User, WatchedProduct};
use sqlx::postgres::{PgPoolOptions, PgRow};
use sqlx::{PgPool, Row};
use std::str::FromStr;

use crate::repo::{CatalogEntry, HistoryPoint, NewUser, PriceRepo, StorageError};

#[derive(Clone)]
pub struct PostgresRepo {
    pool: PgPool,
}

impl PostgresRepo {
    pub async fn connect(url: &str) -> Result<Self, StorageError> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(url)
            .await?;
        Ok(Self { pool })
    }
}

fn row_to_booking(r: &PgRow) -> Result<Booking, sqlx::Error> {
    let brand_s: String = r.try_get("brand")?;
    Ok(Booking {
        reservation_id: r.try_get("reservation_id")?,
        brand: Brand::from_str(&brand_s).map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        account_id: r.try_get("account_id")?,
        ship_code: r.try_get("ship_code")?,
        sail_date: r.try_get("sail_date")?,
        passenger_id: r.try_get("passenger_id")?,
        user_id: r.try_get("user_id")?,
        nights: r.try_get("nights")?,
        package_code: r.try_get("package_code")?,
    })
}

fn row_to_user_pg(r: &PgRow) -> Result<User, StorageError> {
    let brand_s: String = r.try_get("brand_pref")?;
    Ok(User {
        id: r.try_get("id")?,
        telegram_chat_id: r.try_get("telegram_chat_id")?,
        telegram_username: r.try_get("telegram_username")?,
        rcg_username: r.try_get("rcg_username")?,
        rcg_password_ct: r.try_get("rcg_password_ct")?,
        rcg_password_nonce: r.try_get("rcg_password_nonce")?,
        brand_pref: Brand::from_str(&brand_s).map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        active: r.try_get("active")?,
    })
}

#[async_trait]
impl PriceRepo for PostgresRepo {
    async fn migrate(&self) -> Result<(), StorageError> {
        sqlx::migrate!("../../migrations/postgres")
            .run(&self.pool)
            .await?;
        Ok(())
    }

    async fn upsert_user(&self, u: &NewUser<'_>) -> Result<i64, StorageError> {
        let row = sqlx::query(
            r#"
            INSERT INTO users (telegram_chat_id, telegram_username, rcg_username,
                               rcg_password_ct, rcg_password_nonce, brand_pref, active)
            VALUES ($1, $2, $3, $4, $5, $6::brand_kind, TRUE)
            ON CONFLICT (telegram_chat_id) DO UPDATE SET
                telegram_username = EXCLUDED.telegram_username,
                rcg_username = EXCLUDED.rcg_username,
                rcg_password_ct = EXCLUDED.rcg_password_ct,
                rcg_password_nonce = EXCLUDED.rcg_password_nonce,
                brand_pref = EXCLUDED.brand_pref,
                active = TRUE,
                updated_at = now()
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
                      rcg_password_ct, rcg_password_nonce, brand_pref::text AS brand_pref, active
               FROM users WHERE telegram_chat_id = $1"#,
        )
        .bind(chat_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(row_to_user_pg).transpose()
    }

    async fn list_active_users(&self) -> Result<Vec<User>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT id, telegram_chat_id, telegram_username, rcg_username,
                      rcg_password_ct, rcg_password_nonce, brand_pref::text AS brand_pref, active
               FROM users WHERE active = TRUE"#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_user_pg).collect()
    }

    async fn deactivate_user(&self, chat_id: i64) -> Result<(), StorageError> {
        sqlx::query("UPDATE users SET active = FALSE, updated_at = now() WHERE telegram_chat_id = $1")
            .bind(chat_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_user_brand(&self, chat_id: i64, brand: Brand) -> Result<(), StorageError> {
        sqlx::query("UPDATE users SET brand_pref = $1::brand_kind, updated_at = now() WHERE telegram_chat_id = $2")
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
                                  passenger_id, user_id, nights, package_code)
            VALUES ($1, $2::brand_kind, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (reservation_id) DO UPDATE SET
                brand = EXCLUDED.brand,
                account_id = EXCLUDED.account_id,
                ship_code = EXCLUDED.ship_code,
                sail_date = EXCLUDED.sail_date,
                passenger_id = EXCLUDED.passenger_id,
                user_id = EXCLUDED.user_id,
                nights = EXCLUDED.nights,
                package_code = EXCLUDED.package_code,
                updated_at = now()
            "#,
        )
        .bind(&b.reservation_id)
        .bind(b.brand.as_str())
        .bind(&b.account_id)
        .bind(&b.ship_code)
        .bind(b.sail_date)
        .bind(&b.passenger_id)
        .bind(b.user_id)
        .bind(b.nights)
        .bind(&b.package_code)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_bookings(&self) -> Result<Vec<Booking>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT reservation_id, brand::text AS brand, account_id, ship_code,
                      sail_date, passenger_id, user_id, nights, package_code
               FROM bookings ORDER BY sail_date"#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_booking).collect::<Result<_, _>>().map_err(Into::into)
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
            VALUES ($1, $2, $3, $4, TRUE)
            ON CONFLICT (reservation_id, category_prefix, product_code) DO UPDATE SET
                label = COALESCE(EXCLUDED.label, products_watched.label),
                active = TRUE
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
            r#"SELECT id, reservation_id, category_prefix, product_code, label, active
               FROM products_watched WHERE active = TRUE"#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(|r| -> Result<WatchedProduct, sqlx::Error> {
                Ok(WatchedProduct {
                    id: r.try_get("id")?,
                    reservation_id: r.try_get("reservation_id")?,
                    category_prefix: r.try_get("category_prefix")?,
                    product_code: r.try_get("product_code")?,
                    label: r.try_get("label")?,
                    active: r.try_get("active")?,
                })
            })
            .collect::<Result<_, _>>()
            .map_err(Into::into)
    }

    async fn insert_snapshot(&self, s: &PriceSnapshot) -> Result<i64, StorageError> {
        let row = sqlx::query(
            r#"
            INSERT INTO price_snapshots (watched_id, fetched_at, adult_promo_price, child_promo_price, raw_response)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id
            "#,
        )
        .bind(s.watched_id)
        .bind(s.fetched_at)
        .bind(s.adult_promo_price)
        .bind(s.child_promo_price)
        .bind(&s.raw_response)
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
            WHERE watched_id = $1
            ORDER BY fetched_at DESC
            LIMIT 1
            "#,
        )
        .bind(watched_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| {
            Ok::<_, StorageError>(PriceSnapshot {
                watched_id: r.try_get("watched_id")?,
                fetched_at: r.try_get("fetched_at")?,
                adult_promo_price: r.try_get("adult_promo_price")?,
                child_promo_price: r.try_get("child_promo_price")?,
                raw_response: r.try_get("raw_response")?,
            })
        })
        .transpose()
    }

    async fn insert_diff(&self, d: &Diff) -> Result<i64, StorageError> {
        let row = sqlx::query(
            r#"
            INSERT INTO diffs (watched_id, detected_at, old_price, new_price, delta_pct, notified)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id
            "#,
        )
        .bind(d.watched_id)
        .bind(d.detected_at)
        .bind(d.old_price)
        .bind(d.new_price)
        .bind(d.delta_pct)
        .bind(d.notified)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get::<i64, _>("id")?)
    }

    async fn unnotified_diffs(&self) -> Result<Vec<Diff>, StorageError> {
        let rows = sqlx::query(
            r#"
            SELECT id, watched_id, detected_at, old_price, new_price, delta_pct, notified
            FROM diffs
            WHERE notified = FALSE
            ORDER BY detected_at
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(|r| -> Result<Diff, StorageError> {
                Ok(Diff {
                    watched_id: r.try_get("watched_id")?,
                    detected_at: r.try_get("detected_at")?,
                    old_price: r.try_get("old_price")?,
                    new_price: r.try_get("new_price")?,
                    delta_pct: r.try_get("delta_pct")?,
                    notified: r.try_get("notified")?,
                })
            })
            .collect()
    }

    async fn mark_notified(&self, ids: &[i64]) -> Result<(), StorageError> {
        if ids.is_empty() {
            return Ok(());
        }
        sqlx::query("UPDATE diffs SET notified = TRUE WHERE id = ANY($1)")
            .bind(ids)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn upsert_catalog_entry(&self, e: &CatalogEntry) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            INSERT INTO catalog_cache (reservation_id, category_id, category_name,
                                       product_code, title, summary, starting_price, currency,
                                       price_label, base_price_label, unit_label)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (reservation_id, category_id, product_code) DO UPDATE SET
                category_name    = EXCLUDED.category_name,
                title            = EXCLUDED.title,
                summary          = EXCLUDED.summary,
                starting_price   = EXCLUDED.starting_price,
                currency         = EXCLUDED.currency,
                price_label      = EXCLUDED.price_label,
                base_price_label = EXCLUDED.base_price_label,
                unit_label       = EXCLUDED.unit_label,
                fetched_at       = now()
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
        let rows = sqlx::query(
            r#"SELECT reservation_id, category_id, category_name, product_code,
                      title, summary, starting_price, currency,
                      price_label, base_price_label, unit_label
               FROM catalog_cache
               WHERE title ILIKE $1
               ORDER BY ts_rank(to_tsvector('english', title), plainto_tsquery('english', $2)) DESC,
                        title
               LIMIT $3"#,
        )
        .bind(format!("%{}%", q.replace('%', "\\%")))
        .bind(q)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_catalog_pg).collect()
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
               WHERE reservation_id = $1
               ORDER BY category_name, title"#,
        )
        .bind(reservation_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_catalog_pg).collect()
    }

    async fn snapshot_history(
        &self,
        watched_id: i64,
        limit: i64,
    ) -> Result<Vec<HistoryPoint>, StorageError> {
        let rows = sqlx::query(
            r#"SELECT fetched_at, adult_promo_price
               FROM price_snapshots
               WHERE watched_id = $1
               ORDER BY fetched_at DESC
               LIMIT $2"#,
        )
        .bind(watched_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| -> Result<HistoryPoint, StorageError> {
                Ok(HistoryPoint {
                    fetched_at: r.try_get("fetched_at")?,
                    adult_promo_price: r.try_get("adult_promo_price")?,
                })
            })
            .collect()
    }
}

fn row_to_catalog_pg(r: &PgRow) -> Result<CatalogEntry, StorageError> {
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
