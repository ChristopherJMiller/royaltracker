//! Postgres migrations 0012-0013. Skips unless PG_TEST_URL points at an empty
//! scratch database.
#![cfg(feature = "postgres")]

use sqlx::postgres::PgPoolOptions;

#[tokio::test]
async fn legacy_user_becomes_rcg_account_and_combo_voyages_coexist() {
    let Ok(url) = std::env::var("PG_TEST_URL") else {
        eprintln!("PG_TEST_URL not set; skipping");
        return;
    };
    let pool = PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();

    // Migrate to 0011 first, insert a legacy user + booking + deck plan, then the rest.
    let full = sqlx::migrate!("../../migrations/postgres");
    let mut legacy = sqlx::migrate!("../../migrations/postgres");
    legacy.migrations = legacy
        .migrations
        .iter()
        .filter(|m| m.version <= 11)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    legacy.run(&pool).await.unwrap();

    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (telegram_chat_id, rcg_username, rcg_password_ct, rcg_password_nonce) \
         VALUES (42, 'legacy@example.com', '\\x6374', '\\x6e6f6e') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO bookings (reservation_id, brand, account_id, ship_code, sail_date, user_id) \
         VALUES ('R1', 'royal', 'A1', 'OA', '2027-05-07', $1)",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO deck_plans (ship_code, deck, image_url) VALUES ('OA', 5, 'http://x/5.png')")
        .execute(&pool)
        .await
        .unwrap();

    full.run(&pool).await.unwrap();

    let (line, login, ct): (String, String, Vec<u8>) = sqlx::query_as(
        "SELECT cruise_line, login, password_ct FROM cruise_accounts WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (line.as_str(), login.as_str(), ct),
        ("rcg", "legacy@example.com", b"ct".to_vec())
    );
    let linked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM bookings WHERE sailing_id IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(linked, 1);
    let decks: i64 = sqlx::query_scalar("SELECT count(*) FROM deck_plans")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(decks, 0);

    for code in ["4702", "4702A"] {
        sqlx::query(
            "INSERT INTO sailings (brand, ship_code, sail_date, voyage_code) \
             VALUES ('princess', 'ST', '2027-01-02', $1)",
        )
        .bind(code)
        .execute(&pool)
        .await
        .unwrap();
    }
    assert!(sqlx::query(
        "INSERT INTO sailings (brand, ship_code, sail_date, voyage_code) \
         VALUES ('princess', 'ST', '2027-01-02', '4702')"
    )
    .execute(&pool)
    .await
    .is_err());
}
