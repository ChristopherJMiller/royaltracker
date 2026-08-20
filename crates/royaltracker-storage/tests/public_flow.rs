//! End-to-end DB round-trip for the public tier: sailing → tracked product →
//! snapshot → channel → subscription → fan-out → device manage → deactivate.
//! Runs only under the sqlite feature.
#![cfg(feature = "sqlite")]

use chrono::{NaiveDate, Utc};
use royaltracker_storage::{connect, NewPublicChannel, PriceRepo, SailingSnapshot};
use royaltracker_types::{AlertMode, Brand, PublicChannelKind};

async fn repo() -> impl PriceRepo {
    let r = connect("sqlite::memory:").await.unwrap();
    r.migrate().await.unwrap();
    r
}

#[tokio::test]
async fn public_subscription_roundtrip() {
    let r = repo().await;
    let date = NaiveDate::from_ymd_opt(2027, 5, 7).unwrap();

    // Sailing + tracked product (public scope).
    let sailing_id = r.upsert_sailing(Brand::Royal, "OA", date).await.unwrap();
    assert!(r.get_sailing(Brand::Royal, "OA", date).await.unwrap().is_some());
    let tracked_id = r
        .upsert_tracked_product(sailing_id, "3222", "beverage", Some("Deluxe Beverage"), None)
        .await
        .unwrap();
    // Idempotent: same (sailing, product, scope) → same row.
    let again = r
        .upsert_tracked_product(sailing_id, "3222", "beverage", None, None)
        .await
        .unwrap();
    assert_eq!(tracked_id, again);

    // Record a price, then it shows in the public lookup.
    r.record_public_price(
        tracked_id,
        &SailingSnapshot {
            tracked_id,
            fetched_at: Utc::now(),
            adult_promo_price: Some(87.99),
            child_promo_price: None,
            base_price: Some(110.0),
            promo_present: true,
            raw_response: serde_json::Value::Null,
        },
    )
    .await
    .unwrap();
    let prices = r.latest_sailing_prices(Brand::Royal, "OA", date).await.unwrap();
    assert_eq!(prices.len(), 1);
    assert_eq!(prices[0].promo_price, Some(87.99));

    // Channel + subscription.
    let channel_id = r
        .upsert_public_channel(&NewPublicChannel {
            kind: PublicChannelKind::Email,
            endpoint: "erin@example.com",
            p256dh: None,
            auth: None,
            device_token: Some("dev-abc"),
            verified: false,
        })
        .await
        .unwrap();
    let sub_id = r
        .subscribe_public(channel_id, tracked_id, AlertMode::AnyDrop, None)
        .await
        .unwrap();

    // Fan-out sees the subscriber.
    let subs = r.list_public_subscriptions_for(tracked_id).await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].endpoint, "erin@example.com");

    // Device manage sees exactly this subscription.
    let mine = r.list_public_subscriptions_for_device("dev-abc").await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].subscription_id, sub_id);
    assert_eq!(mine[0].latest_promo_price, Some(87.99));

    // Device-scoped deactivate: wrong device can't remove it; right one can.
    assert!(!r.deactivate_public_subscription(sub_id, "someone-else").await.unwrap());
    assert!(r.deactivate_public_subscription(sub_id, "dev-abc").await.unwrap());
    assert!(r.list_public_subscriptions_for(tracked_id).await.unwrap().is_empty());
}

#[tokio::test]
async fn fetch_failure_increments_counter() {
    let r = repo().await;
    let date = NaiveDate::from_ymd_opt(2027, 1, 1).unwrap();
    let sid = r.upsert_sailing(Brand::Celebrity, "AX", date).await.unwrap();
    let tid = r.upsert_tracked_product(sid, "Y8UG", "drinks", None, None).await.unwrap();

    // A tracked product with no snapshot is due to be scraped.
    let due = r.list_sailings_to_scrape().await.unwrap();
    assert!(due.iter().any(|t| t.tracked_id == tid));

    r.note_fetch_failure(tid, "boom").await.unwrap();
    r.note_fetch_failure(tid, "boom2").await.unwrap();
    let due2 = r.list_sailings_to_scrape().await.unwrap();
    let t = due2.iter().find(|t| t.tracked_id == tid).unwrap();
    assert_eq!(t.consecutive_failures, 2);

    // A successful record resets the counter.
    r.record_public_price(
        tid,
        &SailingSnapshot {
            tracked_id: tid,
            fetched_at: Utc::now(),
            adult_promo_price: Some(100.0),
            child_promo_price: None,
            base_price: Some(120.0),
            promo_present: true,
            raw_response: serde_json::Value::Null,
        },
    )
    .await
    .unwrap();
    let due3 = r.list_sailings_to_scrape().await.unwrap();
    let t3 = due3.iter().find(|t| t.tracked_id == tid).unwrap();
    assert_eq!(t3.consecutive_failures, 0);
}
