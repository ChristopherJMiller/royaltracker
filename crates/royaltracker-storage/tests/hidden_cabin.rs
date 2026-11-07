//! A guarantee cabin whose real room Royal masks: we keep the last room seen
//! and date when it went hidden. Runs under the sqlite feature (a fresh file DB
//! per test) and under the postgres feature when PG_TEST_URL points at a
//! scratch database (run with `--test-threads=1`: tests share the database).
#![cfg(any(feature = "sqlite", feature = "postgres"))]

use chrono::NaiveDate;
use royaltracker_storage::{connect, DefaultRepo, PriceRepo};
use royaltracker_types::{Booking, Brand};

const RES: &str = "7654321";

#[cfg(feature = "sqlite")]
fn db_url() -> Option<String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("rt-hidden-{}-{nanos}.db", std::process::id()));
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

async fn repo() -> Option<DefaultRepo> {
    let url = db_url()?;
    let r = connect(&url).await.unwrap();
    r.migrate().await.unwrap();
    #[cfg(feature = "sqlite")]
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    #[cfg(feature = "postgres")]
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::query(&format!("DELETE FROM bookings WHERE reservation_id = '{RES}'"))
        .execute(&pool)
        .await
        .unwrap();
    Some(r)
}

fn day(d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
}

/// A GTY booking as discovery reports it.
fn gty(assigned: Option<&str>, hidden_since: Option<NaiveDate>) -> Booking {
    Booking {
        reservation_id: RES.into(),
        brand: Brand::Royal,
        account_id: "acct".into(),
        ship_code: "QQ".into(),
        sail_date: NaiveDate::from_ymd_opt(2099, 1, 1).unwrap(),
        passenger_id: Some("p1".into()),
        nights: Some(7),
        package_code: None,
        stateroom: Some("GTY".into()),
        assigned_stateroom: assigned.map(str::to_string),
        assigned_hidden_since: hidden_since,
        voyage_code: None,
        currency: None,
        deck_name: None,
        cabin_category: None,
    }
}

async fn stored(r: &DefaultRepo) -> Booking {
    r.list_bookings()
        .await
        .unwrap()
        .into_iter()
        .find(|b| b.reservation_id == RES)
        .unwrap()
}

#[tokio::test]
async fn hidden_keeps_last_room_and_first_date() {
    let Some(r) = repo().await else { return };
    r.upsert_booking(&gty(Some("1234"), None)).await.unwrap();
    r.upsert_booking(&gty(None, Some(day(4)))).await.unwrap();
    r.upsert_booking(&gty(None, Some(day(5)))).await.unwrap();
    let b = stored(&r).await;
    assert_eq!(b.assigned_stateroom.as_deref(), Some("1234"));
    assert_eq!(b.assigned_hidden_since, Some(day(4)));
}

#[tokio::test]
async fn failed_read_keeps_hidden_date() {
    let Some(r) = repo().await else { return };
    r.upsert_booking(&gty(Some("1234"), None)).await.unwrap();
    r.upsert_booking(&gty(None, Some(day(4)))).await.unwrap();
    r.upsert_booking(&gty(None, None)).await.unwrap();
    let b = stored(&r).await;
    assert_eq!(b.assigned_stateroom.as_deref(), Some("1234"));
    assert_eq!(b.assigned_hidden_since, Some(day(4)));
}

#[tokio::test]
async fn real_room_clears_hidden_date() {
    let Some(r) = repo().await else { return };
    r.upsert_booking(&gty(Some("1234"), None)).await.unwrap();
    r.upsert_booking(&gty(None, Some(day(4)))).await.unwrap();
    r.upsert_booking(&gty(Some("5678"), None)).await.unwrap();
    let b = stored(&r).await;
    assert_eq!(b.assigned_stateroom.as_deref(), Some("5678"));
    assert_eq!(b.assigned_hidden_since, None);
}

#[tokio::test]
async fn official_assignment_clears_hidden_date() {
    let Some(r) = repo().await else { return };
    r.upsert_booking(&gty(Some("1234"), None)).await.unwrap();
    r.upsert_booking(&gty(None, Some(day(4)))).await.unwrap();
    let mut assigned = gty(None, None);
    assigned.stateroom = Some("4321".into());
    r.upsert_booking(&assigned).await.unwrap();
    let b = stored(&r).await;
    assert_eq!(b.stateroom.as_deref(), Some("4321"));
    assert_eq!(b.assigned_hidden_since, None);
}
