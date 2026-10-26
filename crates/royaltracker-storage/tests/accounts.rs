//! Cruise accounts, the cross-line booking guard, voyage-keyed sailings and
//! user-scoped catalog search. Runs only under the sqlite feature.
#![cfg(feature = "sqlite")]

use chrono::NaiveDate;
use royaltracker_storage::{
    connect, CatalogEntry, NewCruiseAccount, NewUser, PriceRepo, SailingKey, StorageError,
};
use royaltracker_types::{Booking, Brand, CruiseLine};

async fn repo() -> impl PriceRepo {
    let r = connect("sqlite::memory:").await.unwrap();
    r.migrate().await.unwrap();
    r
}

async fn user(r: &impl PriceRepo, chat: i64) -> i64 {
    r.upsert_user(&NewUser {
        telegram_chat_id: chat,
        telegram_username: Some("tester"),
    })
    .await
    .unwrap()
}

fn acct<'a>(user_id: i64, line: CruiseLine, login: &'a str, ct: &'a [u8]) -> NewCruiseAccount<'a> {
    NewCruiseAccount {
        user_id,
        line,
        login,
        password_ct: ct,
        password_nonce: b"nonce",
    }
}

fn booking(id: &str, brand: Brand, voyage: Option<&str>, currency: Option<&str>) -> Booking {
    Booking {
        reservation_id: id.to_string(),
        brand,
        account_id: "acct".to_string(),
        ship_code: "XX".to_string(),
        sail_date: NaiveDate::from_ymd_opt(2027, 3, 1).unwrap(),
        passenger_id: None,
        nights: Some(7),
        package_code: None,
        stateroom: None,
        assigned_stateroom: None,
        voyage_code: voyage.map(str::to_string),
        currency: currency.map(str::to_string),
        deck_name: None,
        cabin_category: None,
    }
}

#[tokio::test]
async fn upsert_account_replaces_creds_and_clears_error() {
    let r = repo().await;
    let uid = user(&r, 1).await;
    let id1 = r.upsert_cruise_account(&acct(uid, CruiseLine::Rcg, "a@x.com", b"ct1")).await.unwrap();
    r.record_login_outcome(id1, Some("bad")).await.unwrap();
    let mid = r.list_cruise_accounts(uid).await.unwrap();
    assert_eq!(mid[0].last_login_error.as_deref(), Some("bad"));
    assert!(mid[0].last_login_at.is_some());

    let id2 = r.upsert_cruise_account(&acct(uid, CruiseLine::Rcg, "b@x.com", b"ct2")).await.unwrap();
    assert_eq!(id1, id2);
    let accts = r.list_cruise_accounts(uid).await.unwrap();
    assert_eq!(accts.len(), 1);
    assert_eq!(accts[0].login, "b@x.com");
    assert_eq!(accts[0].password_ct, b"ct2".to_vec());
    assert_eq!(accts[0].last_login_error, None);
    assert!(accts[0].active);
}

#[tokio::test]
async fn same_login_two_users() {
    let r = repo().await;
    let u1 = user(&r, 1).await;
    let u2 = user(&r, 2).await;
    r.upsert_cruise_account(&acct(u1, CruiseLine::Rcg, "same@x.com", b"c")).await.unwrap();
    r.upsert_cruise_account(&acct(u2, CruiseLine::Rcg, "same@x.com", b"c")).await.unwrap();
    assert_eq!(r.list_active_cruise_accounts().await.unwrap().len(), 2);

    r.upsert_booking(&booking("1234567", Brand::Royal, None, None)).await.unwrap();
    r.subscribe_user_to_booking("1234567", u1).await.unwrap();
    r.subscribe_user_to_booking("1234567", u2).await.unwrap();
    assert_eq!(r.list_subscribers_for_reservation("1234567").await.unwrap().len(), 2);
}

#[tokio::test]
async fn remove_account_unsubscribes_line_bookings() {
    let r = repo().await;
    let uid = user(&r, 1).await;
    r.upsert_cruise_account(&acct(uid, CruiseLine::Rcg, "a@x.com", b"c")).await.unwrap();
    r.upsert_cruise_account(&acct(uid, CruiseLine::Princess, "p@x.com", b"c")).await.unwrap();
    r.upsert_booking(&booking("1111111", Brand::Royal, None, None)).await.unwrap();
    r.upsert_booking(&booking("2222222", Brand::Princess, Some("X637"), Some("USD"))).await.unwrap();
    r.subscribe_user_to_booking("1111111", uid).await.unwrap();
    r.subscribe_user_to_booking("2222222", uid).await.unwrap();

    assert!(r.remove_cruise_account(uid, CruiseLine::Rcg).await.unwrap());
    let left = r.list_bookings_for_user(uid).await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].reservation_id, "2222222");
    let accts = r.list_cruise_accounts(uid).await.unwrap();
    assert_eq!(accts.len(), 1);
    assert_eq!(accts[0].line, CruiseLine::Princess);

    assert!(!r.remove_cruise_account(uid, CruiseLine::Rcg).await.unwrap());
}

#[tokio::test]
async fn inactive_user_accounts_not_listed() {
    let r = repo().await;
    let uid = user(&r, 7).await;
    r.upsert_cruise_account(&acct(uid, CruiseLine::Rcg, "a@x.com", b"c")).await.unwrap();
    let active = r.list_active_cruise_accounts().await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].user.id, uid);
    assert_eq!(active[0].account.login, "a@x.com");

    r.deactivate_user(7).await.unwrap();
    assert!(r.list_active_cruise_accounts().await.unwrap().is_empty());
}

#[tokio::test]
async fn deactivate_user_deletes_all_accounts() {
    let r = repo().await;
    let uid = user(&r, 8).await;
    let other = user(&r, 9).await;
    r.upsert_cruise_account(&acct(uid, CruiseLine::Rcg, "a@x.com", b"c")).await.unwrap();
    r.upsert_cruise_account(&acct(uid, CruiseLine::Princess, "p@x.com", b"c")).await.unwrap();
    r.upsert_cruise_account(&acct(other, CruiseLine::Rcg, "o@x.com", b"c")).await.unwrap();
    assert_eq!(r.list_cruise_accounts(uid).await.unwrap().len(), 2);

    r.deactivate_user(8).await.unwrap();
    assert!(r.list_cruise_accounts(uid).await.unwrap().is_empty());

    // Re-registering (any line) must not resurrect the deleted logins.
    let again = user(&r, 8).await;
    assert_eq!(again, uid);
    assert!(r.list_cruise_accounts(uid).await.unwrap().is_empty());
    let active = r.list_active_cruise_accounts().await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].user.id, other);
    // Other users are untouched.
    assert_eq!(r.list_cruise_accounts(other).await.unwrap().len(), 1);
}

#[tokio::test]
async fn cross_line_booking_never_overwrites() {
    let r = repo().await;
    r.upsert_booking(&booking("1234567", Brand::Royal, None, None)).await.unwrap();
    let err = r
        .upsert_booking(&booking("1234567", Brand::Princess, Some("X637"), Some("USD")))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StorageError::CrossLineConflict { reservation_id } if reservation_id == "1234567"),
        "got {err:?}"
    );
    let all = r.list_bookings().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].brand, Brand::Royal);
    assert_eq!(all[0].voyage_code, None);

    // Same-line brand change (Royal -> Celebrity) is still allowed.
    r.upsert_booking(&booking("1234567", Brand::Celebrity, None, None)).await.unwrap();
    assert_eq!(r.list_bookings().await.unwrap()[0].brand, Brand::Celebrity);
}

#[tokio::test]
async fn upsert_booking_links_sailing() {
    let r = repo().await;
    r.upsert_booking(&booking("9999999", Brand::Princess, Some("X637"), Some("USD")))
        .await
        .unwrap();
    let date = NaiveDate::from_ymd_opt(2027, 3, 1).unwrap();
    let s = r
        .get_sailing(&SailingKey {
            brand: Brand::Princess,
            ship_code: "XX",
            sail_date: date,
            voyage_code: Some("X637"),
        })
        .await
        .unwrap()
        .expect("sailing linked");
    assert_eq!(s.currency.as_deref(), Some("USD"));
    assert_eq!(s.voyage_code.as_deref(), Some("X637"));
    // The empty-voyage sailing is a different row.
    assert!(r
        .get_sailing(&SailingKey { brand: Brand::Princess, ship_code: "XX", sail_date: date, voyage_code: None })
        .await
        .unwrap()
        .is_none());
    let b = &r.list_bookings().await.unwrap()[0];
    assert_eq!(b.voyage_code.as_deref(), Some("X637"));
    assert_eq!(b.currency.as_deref(), Some("USD"));
}

fn entry(res: &str, title: &str) -> CatalogEntry {
    CatalogEntry {
        reservation_id: res.to_string(),
        category_id: "cat".to_string(),
        category_name: "Drinks".to_string(),
        product_code: format!("p-{res}"),
        title: title.to_string(),
        summary: None,
        starting_price: None,
        currency: None,
        price_label: None,
        base_price_label: None,
        unit_label: None,
    }
}

#[tokio::test]
async fn search_catalog_scoped_to_user() {
    let r = repo().await;
    let a = user(&r, 1).await;
    let b = user(&r, 2).await;
    r.upsert_booking(&booking("AAA1111", Brand::Royal, None, None)).await.unwrap();
    r.upsert_booking(&booking("BBB2222", Brand::Royal, None, None)).await.unwrap();
    r.subscribe_user_to_booking("AAA1111", a).await.unwrap();
    r.subscribe_user_to_booking("BBB2222", b).await.unwrap();
    r.upsert_catalog_entry(&entry("AAA1111", "Soda Package")).await.unwrap();
    r.upsert_catalog_entry(&entry("BBB2222", "Soda Package")).await.unwrap();

    let hits = r.search_catalog(a, "soda", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reservation_id, "AAA1111");
    let hits = r.search_catalog(b, "soda", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reservation_id, "BBB2222");
}

#[tokio::test]
async fn cabin_details_round_trip_and_survive_a_blank_refresh() {
    let r = repo().await;
    let uid = user(&r, 1).await;
    let mut b = booking("AB12CD", Brand::Princess, Some("X637"), Some("USD"));
    b.stateroom = Some("D123".into());
    b.deck_name = Some("Dolphin".into());
    b.cabin_category = Some("Deluxe Balcony".into());
    r.upsert_booking(&b).await.unwrap();
    r.subscribe_user_to_booking("AB12CD", uid).await.unwrap();

    // A later refresh that couldn't read the cabin keeps what we had.
    let mut blank = b.clone();
    blank.stateroom = None;
    blank.deck_name = None;
    blank.cabin_category = None;
    r.upsert_booking(&blank).await.unwrap();

    let got = r.list_bookings_for_user(uid).await.unwrap();
    assert_eq!(got[0].stateroom.as_deref(), Some("D123"));
    assert_eq!(got[0].deck_name.as_deref(), Some("Dolphin"));
    assert_eq!(got[0].cabin_category.as_deref(), Some("Deluxe Balcony"));
    let all = r.list_bookings().await.unwrap();
    assert_eq!(all[0].deck_name.as_deref(), Some("Dolphin"));
}
