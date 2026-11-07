//! Booked items (what a cabin bought), paid-price alerts and watch dismissals.
//! Runs under the sqlite feature (a fresh file DB per test) and under the
//! postgres feature when PG_TEST_URL points at a scratch database (run with
//! `--test-threads=1`: tests share the database and reset their booking).
#![cfg(any(feature = "sqlite", feature = "postgres"))]

use chrono::{NaiveDate, TimeZone, Utc};
use royaltracker_storage::{connect, DefaultRepo, PriceRepo};
use royaltracker_types::{AlertMode, BookedItem, Booking, Brand, PaidUnit, WatchedProduct};

const RES: &str = "1234567";

#[cfg(feature = "sqlite")]
fn db_url() -> Option<String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("rt-booked-{}-{nanos}.db", std::process::id()));
    Some(format!("sqlite://{}", path.display()))
}

#[cfg(feature = "postgres")]
fn db_url() -> Option<String> {
    let url = std::env::var("PG_TEST_URL").ok();
    if url.is_none() {
        eprintln!("PG_TEST_URL not set; skipping");
    }
    url
}

/// Run raw SQL outside the repo, to set up legacy state.
async fn raw_exec(url: &str, sql: &str) {
    #[cfg(feature = "sqlite")]
    let pool = sqlx::SqlitePool::connect(url).await.unwrap();
    #[cfg(feature = "postgres")]
    let pool = sqlx::PgPool::connect(url).await.unwrap();
    sqlx::query(sql).execute(&pool).await.unwrap();
}

async fn repo() -> Option<(DefaultRepo, String)> {
    let url = db_url()?;
    let r = connect(&url).await.unwrap();
    r.migrate().await.unwrap();
    // Cascades to its watches and booked items (a shared Postgres DB).
    raw_exec(&url, &format!("DELETE FROM bookings WHERE reservation_id = '{RES}'")).await;
    r.upsert_booking(&Booking {
        reservation_id: RES.into(),
        brand: Brand::Royal,
        account_id: "acct".into(),
        ship_code: "QQ".into(),
        sail_date: NaiveDate::from_ymd_opt(2099, 1, 1).unwrap(),
        passenger_id: Some("p1".into()),
        nights: Some(7),
        package_code: None,
        stateroom: None,
        assigned_stateroom: None,
        voyage_code: None,
        currency: None,
        deck_name: None,
        cabin_category: None,
    })
    .await
    .unwrap();
    Some((r, url))
}

fn item(code: &str, paid_total: f64) -> BookedItem {
    BookedItem {
        reservation_id: RES.into(),
        category_prefix: "beverage".into(),
        product_code: code.into(),
        title: format!("Item {code}"),
        unit: PaidUnit::Day,
        adults: 2,
        children: 0,
        paid_total,
        paid_each: Some(paid_total / 2.0),
        currency: Some("USD".into()),
        last_alert_total: None,
        orders_read_at: Utc.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap(),
    }
}

async fn active_watch(r: &DefaultRepo, code: &str) -> Option<WatchedProduct> {
    r.list_active_watched()
        .await
        .unwrap()
        .into_iter()
        .find(|w| w.reservation_id == RES && w.product_code == code)
}

#[tokio::test]
async fn replace_reports_added_and_removed() {
    let Some((r, _)) = repo().await else { return };
    let c = r.replace_booked_items(RES, &[item("A", 10.0), item("B", 20.0)]).await.unwrap();
    let mut added: Vec<_> = c.added.iter().map(|i| i.product_code.as_str()).collect();
    added.sort();
    assert_eq!(added, ["A", "B"]);
    assert!(c.removed.is_empty());

    let c = r.replace_booked_items(RES, &[item("B", 20.0), item("C", 30.0)]).await.unwrap();
    assert_eq!(c.added.iter().map(|i| i.product_code.as_str()).collect::<Vec<_>>(), ["C"]);
    assert_eq!(c.removed, ["A"]);

    let stored = r.list_booked_items(RES).await.unwrap();
    let mut codes: Vec<_> = stored.iter().map(|i| i.product_code.as_str()).collect();
    codes.sort();
    assert_eq!(codes, ["B", "C"]);
    let b = stored.iter().find(|i| i.product_code == "B").unwrap();
    assert_eq!(b, &item("B", 20.0));
}

#[tokio::test]
async fn changed_paid_total_resets_last_alert() {
    let Some((r, _)) = repo().await else { return };
    r.replace_booked_items(RES, &[item("B", 145.98)]).await.unwrap();
    r.set_booked_last_alert(RES, "B", 137.98).await.unwrap();

    r.replace_booked_items(RES, &[item("B", 145.98)]).await.unwrap();
    assert_eq!(r.list_booked_items(RES).await.unwrap()[0].last_alert_total, Some(137.98));

    r.replace_booked_items(RES, &[item("B", 137.98)]).await.unwrap();
    assert_eq!(r.list_booked_items(RES).await.unwrap()[0].last_alert_total, None);
}

#[tokio::test]
async fn ensure_creates_tracked_watch() {
    let Some((r, _)) = repo().await else { return };
    let id = r.ensure_booked_watch(RES, "beverage", "B", "Item B").await.unwrap();
    let w = active_watch(&r, "B").await.unwrap();
    assert_eq!(Some(w.id), id);
    assert!(w.alert_on_paid);
    assert_eq!(w.label.as_deref(), Some("Item B"));
}

#[tokio::test]
async fn ensure_switches_existing_watch_to_paid() {
    let Some((r, _)) = repo().await else { return };
    let id = r.upsert_watched(RES, "beverage", "B", None).await.unwrap();
    r.set_watch_alert(id, AlertMode::BelowThreshold, Some(68.0)).await.unwrap();
    assert_eq!(r.ensure_booked_watch(RES, "beverage", "B", "Item B").await.unwrap(), Some(id));
    assert!(active_watch(&r, "B").await.unwrap().alert_on_paid);
}

#[tokio::test]
async fn dismissed_watch_not_readded() {
    let Some((r, _)) = repo().await else { return };
    let id = r.upsert_watched(RES, "beverage", "B", None).await.unwrap();
    r.deactivate_watched(id).await.unwrap();
    assert_eq!(r.ensure_booked_watch(RES, "beverage", "B", "Item B").await.unwrap(), None);
    assert!(active_watch(&r, "B").await.is_none());
}

#[tokio::test]
async fn pre_launch_inactive_watch_comes_back() {
    let Some((r, url)) = repo().await else { return };
    let id = r.upsert_watched(RES, "beverage", "B", None).await.unwrap();
    // Removed before this feature shipped: inactive, no dismissal recorded.
    raw_exec(&url, &format!("UPDATE products_watched SET active = FALSE WHERE id = {id}")).await;
    assert_eq!(r.ensure_booked_watch(RES, "beverage", "B", "Item B").await.unwrap(), Some(id));
    assert!(active_watch(&r, "B").await.unwrap().alert_on_paid);
}

#[tokio::test]
async fn readding_after_dismissal_clears_it() {
    let Some((r, _)) = repo().await else { return };
    let id = r.upsert_watched(RES, "beverage", "B", None).await.unwrap();
    r.deactivate_watched(id).await.unwrap();
    r.upsert_watched(RES, "beverage", "B", None).await.unwrap();
    assert_eq!(r.ensure_booked_watch(RES, "beverage", "B", "Item B").await.unwrap(), Some(id));
}

#[tokio::test]
async fn release_returns_to_any_drop() {
    let Some((r, _)) = repo().await else { return };
    r.ensure_booked_watch(RES, "beverage", "B", "Item B").await.unwrap();
    r.release_booked_watch(RES, "B").await.unwrap();
    let w = active_watch(&r, "B").await.unwrap();
    assert!(!w.alert_on_paid);
    assert_eq!(w.alert_mode, AlertMode::AnyDrop);
    assert_eq!(w.alert_threshold, None);
}

#[tokio::test]
async fn release_leaves_manual_alert_alone() {
    let Some((r, _)) = repo().await else { return };
    let id = r.ensure_booked_watch(RES, "beverage", "B", "Item B").await.unwrap().unwrap();
    r.set_watch_alert(id, AlertMode::BelowThreshold, Some(68.0)).await.unwrap();
    r.release_booked_watch(RES, "B").await.unwrap();
    let w = active_watch(&r, "B").await.unwrap();
    assert_eq!(w.alert_mode, AlertMode::BelowThreshold);
    assert_eq!(w.alert_threshold, Some(68.0));
}

#[tokio::test]
async fn manual_alert_clears_paid() {
    let Some((r, _)) = repo().await else { return };
    let id = r.ensure_booked_watch(RES, "beverage", "B", "Item B").await.unwrap().unwrap();
    r.set_watch_alert(id, AlertMode::BelowThreshold, Some(68.0)).await.unwrap();
    assert!(!active_watch(&r, "B").await.unwrap().alert_on_paid);
    r.set_watch_alert_on_paid(id).await.unwrap();
    assert!(active_watch(&r, "B").await.unwrap().alert_on_paid);
}
