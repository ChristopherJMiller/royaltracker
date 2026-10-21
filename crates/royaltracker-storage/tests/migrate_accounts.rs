//! Migrations 0012-0013: legacy users become `rcg` cruise accounts, the
//! users/bookings/sailings rebuild keeps child data, and Princess combo
//! voyages can share ship + date. Assertions use raw SQL on a second pool so
//! they don't depend on repo methods.
#![cfg(feature = "sqlite")]

use royaltracker_storage::{connect, PriceRepo};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

fn temp_db_path() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("rt-migrate-{}-{nanos}.db", std::process::id()))
}

async fn open(path: &std::path::Path) -> SqlitePool {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .unwrap()
        .create_if_missing(true)
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap()
}

/// Migrate a fresh temp DB up to 0011 and return its path.
async fn legacy_db() -> (std::path::PathBuf, SqlitePool) {
    let path = temp_db_path();
    let pool = open(&path).await;
    let mut m = sqlx::migrate!("../../migrations/sqlite");
    m.migrations = m
        .migrations
        .iter()
        .filter(|mg| mg.version <= 11)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    m.run(&pool).await.unwrap();
    (path, pool)
}

#[tokio::test]
async fn legacy_user_becomes_rcg_account_and_data_survives() {
    let (path, pool) = legacy_db().await;

    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (telegram_chat_id, rcg_username, rcg_password_ct, rcg_password_nonce) \
         VALUES (42, 'legacy@example.com', x'6374', x'6e6f6e') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO bookings (reservation_id, brand, account_id, ship_code, sail_date, user_id) \
         VALUES ('R1', 'royal', 'A1', 'OA', '2027-05-07', ?)",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO booking_subscribers (reservation_id, user_id) VALUES ('R1', ?)")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    let watched_id: i64 = sqlx::query_scalar(
        "INSERT INTO products_watched (reservation_id, category_prefix, product_code) \
         VALUES ('R1', 'beverage', '3222') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO price_snapshots (watched_id, fetched_at, raw_response) VALUES (?, '2026-01-01T00:00:00Z', '{}')",
    )
    .bind(watched_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO deck_plans (ship_code, deck, image_url) VALUES ('OA', 5, 'http://x/5.png')")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let repo = connect(&format!("sqlite://{}", path.display())).await.unwrap();
    repo.migrate().await.unwrap();
    // migrate() must restore FK enforcement on the repo's own pool.
    assert!(
        repo.upsert_watched("NO-SUCH-RESERVATION", "beverage", "3222", None)
            .await
            .is_err(),
        "foreign keys must be enforced again after migrate()"
    );
    drop(repo);

    let pool = open(&path).await;
    let (line, login, ct): (String, String, Vec<u8>) = sqlx::query_as(
        "SELECT cruise_line, login, password_ct FROM cruise_accounts WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (line.as_str(), login.as_str(), ct),
        ("rcg", "legacy@example.com", b"ct".to_vec())
    );
    let count = |sql: &'static str| {
        let pool = pool.clone();
        async move { sqlx::query_scalar::<_, i64>(sql).fetch_one(&pool).await.unwrap() }
    };
    assert_eq!(count("SELECT count(*) FROM users").await, 1);
    assert_eq!(count("SELECT count(*) FROM bookings").await, 1);
    assert_eq!(count("SELECT count(*) FROM booking_subscribers").await, 1);
    assert_eq!(count("SELECT count(*) FROM products_watched").await, 1); // rebuild didn't cascade
    assert_eq!(count("SELECT count(*) FROM price_snapshots").await, 1);
    assert_eq!(count("SELECT count(*) FROM bookings WHERE sailing_id IS NOT NULL").await, 1);
    assert_eq!(count("SELECT count(*) FROM deck_plans").await, 0); // cache emptied
    assert_eq!(count("SELECT count(*) FROM pragma_foreign_key_check").await, 0);
    pool.close().await;
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn princess_combo_voyages_share_ship_and_date() {
    let path = temp_db_path();
    let repo = connect(&format!("sqlite://{}", path.display())).await.unwrap();
    repo.migrate().await.unwrap();
    drop(repo);

    let pool = open(&path).await;
    let ins = |code: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO sailings (brand, ship_code, sail_date, voyage_code) \
                 VALUES ('princess', 'ST', '2027-01-02', ?)",
            )
            .bind(code)
            .execute(&pool)
            .await
        }
    };
    ins("4702").await.unwrap();
    ins("4702A").await.unwrap();
    assert!(ins("4702").await.is_err(), "duplicate voyage must violate unique key");
    pool.close().await;
    let _ = std::fs::remove_file(&path);
}
