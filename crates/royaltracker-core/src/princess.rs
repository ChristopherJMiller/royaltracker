//! Princess account discovery: one login, one booking context, persisted.

use crate::bookings::DiscoveryReport;
use royaltracker_api::princess::{PrincessAccountClient, PrincessBookingContext, PrincessGuestClient};
use royaltracker_api::{ApiError, PublicFetch, PublicProduct};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use royaltracker_storage::{PriceRepo, SailingKey};
use royaltracker_types::{Booking, Brand, CruiseAccount, User};
use tracing::warn;

pub const PRINCESS_NO_BOOKING_MSG: &str =
    "Signed in to Princess, but couldn't open a booking — multi-booking accounts aren't supported yet.";

/// Map a Princess booking context to a `Booking`. Only tracking fields are kept.
pub fn booking_from_context(ctx: &PrincessBookingContext, account_id: i64) -> Booking {
    Booking {
        reservation_id: ctx.booking_id.clone(),
        brand: Brand::Princess,
        account_id: format!("princess:{account_id}"),
        ship_code: ctx.ship_code.clone(),
        sail_date: ctx.sail_date,
        passenger_id: Some(ctx.pax_id.clone()),
        nights: ctx.nights,
        package_code: None,
        stateroom: None,
        assigned_stateroom: None,
        voyage_code: Some(ctx.voyage_code.clone()),
        currency: Some(ctx.currency.clone()),
    }
}

/// Log in to Princess once (no retry), persist the booking and subscribe the user.
pub async fn discover_princess(
    repo: &dyn PriceRepo,
    user: &User,
    account: &CruiseAccount,
    password: &str,
) -> anyhow::Result<DiscoveryReport> {
    // One client per call: its cookie jar must not be shared across accounts.
    let client = PrincessAccountClient::new()?;
    let ctx = client.login_and_context(&account.login, password).await?;
    persist_context(repo, user, account, ctx).await
}

pub(crate) async fn persist_context(
    repo: &dyn PriceRepo,
    user: &User,
    account: &CruiseAccount,
    ctx: Option<PrincessBookingContext>,
) -> anyhow::Result<DiscoveryReport> {
    let mut report = DiscoveryReport { logins_ok: 1, ..Default::default() };
    let Some(ctx) = ctx else {
        report.errors.push(PRINCESS_NO_BOOKING_MSG.to_string());
        return Ok(report);
    };
    let booking = booking_from_context(&ctx, account.id);
    let key = SailingKey {
        brand: Brand::Princess,
        ship_code: &ctx.ship_code,
        sail_date: ctx.sail_date,
        voyage_code: Some(&ctx.voyage_code),
    };
    if let Err(e) = repo.upsert_sailing(&key, Some(&ctx.currency)).await {
        warn!(error = %e, "princess upsert_sailing failed");
        report.errors.push(format!("sailing: {e}"));
        return Ok(report);
    }
    if let Err(e) = repo.upsert_booking(&booking).await {
        warn!(error = %e, "princess upsert_booking failed");
        report.errors.push(format!("upsert: {e}"));
        return Ok(report);
    }
    if let Err(e) = repo.subscribe_user_to_booking(&booking.reservation_id, user.id).await {
        warn!(error = %e, "princess subscribe_user_to_booking failed");
        report.errors.push(format!("subscribe: {e}"));
        return Ok(report);
    }
    report.persisted = 1;
    Ok(report)
}


/// Merge excursions and extras into one map keyed by product code. Excursions
/// win on a (theoretical) code collision.
pub fn merge_products(
    excursions: Vec<PublicProduct>,
    extras: Vec<PublicProduct>,
) -> HashMap<String, PublicProduct> {
    let mut m: HashMap<String, PublicProduct> = HashMap::new();
    for p in extras {
        m.insert(p.product_code.clone(), p);
    }
    for p in excursions {
        m.insert(p.product_code.clone(), p);
    }
    m
}

/// Per-run Princess price cache. Each excursions key (voyage, currency) and
/// each extras key (currency) is attempted at most once per cache instance,
/// whether it succeeds or fails (`None` records a failure).
#[derive(Default)]
pub struct PrincessPriceCache {
    excursions: HashMap<(String, String), Option<Vec<PublicProduct>>>,
    extras: HashMap<String, Option<Vec<PublicProduct>>>,
}

impl PrincessPriceCache {
    /// Returns whatever half succeeded; `Err` only when both failed.
    pub async fn products(
        &mut self,
        client: &PrincessGuestClient,
        voyage_code: &str,
        currency: &str,
    ) -> Result<HashMap<String, PublicProduct>, ApiError> {
        let r = self
            .products_with(
                voyage_code,
                currency,
                || client.fetch_excursions(voyage_code, currency),
                || client.fetch_extras(currency),
            )
            .await;
        r.ok_or_else(|| ApiError::Status { status: 502, body: format!("Princess fetch failed for {voyage_code}/{currency}") })
    }

    /// Network-free core: fetch closures run at most once per key. `None` when
    /// both halves failed.
    pub(crate) async fn products_with<E, FE, FX, FutE, FutX>(
        &mut self,
        voyage_code: &str,
        currency: &str,
        fetch_ex: FE,
        fetch_extras: FX,
    ) -> Option<HashMap<String, PublicProduct>>
    where
        E: std::fmt::Display,
        FE: FnOnce() -> FutE,
        FX: FnOnce() -> FutX,
        FutE: std::future::Future<Output = Result<Vec<PublicProduct>, E>>,
        FutX: std::future::Future<Output = Result<Vec<PublicProduct>, E>>,
    {
        let ek = (voyage_code.to_string(), currency.to_string());
        if !self.excursions.contains_key(&ek) {
            let r = match fetch_ex().await {
                Ok(v) => Some(v),
                Err(e) => {
                    warn!(error = %e, voyage = voyage_code, currency, "princess excursions fetch failed");
                    None
                }
            };
            self.excursions.insert(ek.clone(), r);
        }
        if !self.extras.contains_key(currency) {
            let r = match fetch_extras().await {
                Ok(v) => Some(v),
                Err(e) => {
                    warn!(error = %e, currency, "princess extras fetch failed");
                    None
                }
            };
            self.extras.insert(currency.to_string(), r);
        }
        let ex = self.excursions[&ek].clone();
        let xt = self.extras[currency].clone();
        if ex.is_none() && xt.is_none() {
            return None;
        }
        Some(merge_products(ex.unwrap_or_default(), xt.unwrap_or_default()))
    }

    /// Like [`Self::products`], but the fleet-wide extras come from a shared,
    /// TTL'd cache (web paths). Excursions are still fetched per call.
    pub async fn products_shared(
        &mut self,
        client: &PrincessGuestClient,
        extras: &PrincessExtrasCache,
        voyage_code: &str,
        currency: &str,
    ) -> Result<HashMap<String, PublicProduct>, ApiError> {
        let r = self
            .products_with(
                voyage_code,
                currency,
                || client.fetch_excursions(voyage_code, currency),
                || async { extras.get(client, currency).await.map(|v| v.as_ref().clone()) },
            )
            .await;
        r.ok_or_else(|| ApiError::Status { status: 502, body: format!("Princess fetch failed for {voyage_code}/{currency}") })
    }
}

/// How long shared fleet-wide extras stay fresh.
const EXTRAS_TTL: Duration = Duration::from_secs(6 * 3600);
/// After a failed extras fetch, don't refetch before this.
const EXTRAS_RETRY_AFTER: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
enum ExtrasEntry {
    Ok { fetched_at: Instant, products: Arc<Vec<PublicProduct>> },
    Failed { retry_after: Instant },
}

#[derive(Debug, PartialEq, Eq)]
enum ExtrasStep {
    /// Cached and fresh.
    Use,
    /// Absent, expired, or a failure whose retry-after has passed.
    Fetch,
    /// Failed recently: error without fetching.
    Blocked,
}

fn extras_step(entry: Option<&ExtrasEntry>, now: Instant) -> ExtrasStep {
    match entry {
        Some(ExtrasEntry::Ok { fetched_at, .. }) if now.saturating_duration_since(*fetched_at) < EXTRAS_TTL => {
            ExtrasStep::Use
        }
        Some(ExtrasEntry::Failed { retry_after }) if now < *retry_after => ExtrasStep::Blocked,
        _ => ExtrasStep::Fetch,
    }
}

type ExtrasSlot = Arc<tokio::sync::Mutex<Option<ExtrasEntry>>>;

/// Fleet-wide Princess extras per currency, shared across web requests and
/// fresh for 6 h. A failure is remembered only for 15 min. Concurrent callers
/// for one currency share a single fetch. (The scraper keeps its per-run
/// [`PrincessPriceCache`].)
#[derive(Default)]
pub struct PrincessExtrasCache {
    slots: std::sync::Mutex<HashMap<String, ExtrasSlot>>,
}

impl PrincessExtrasCache {
    fn slot(&self, currency: &str) -> ExtrasSlot {
        let mut m = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(currency.to_string()).or_default().clone()
    }

    pub async fn get(
        &self,
        client: &PrincessGuestClient,
        currency: &str,
    ) -> Result<Arc<Vec<PublicProduct>>, ApiError> {
        self.get_with(currency, || client.fetch_extras(currency)).await
    }

    pub(crate) async fn get_with<F, Fut>(
        &self,
        currency: &str,
        fetch: F,
    ) -> Result<Arc<Vec<PublicProduct>>, ApiError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<PublicProduct>, ApiError>>,
    {
        let slot = self.slot(currency);
        let mut entry = slot.lock().await;
        match (extras_step(entry.as_ref(), Instant::now()), entry.as_ref()) {
            (ExtrasStep::Use, Some(ExtrasEntry::Ok { products, .. })) => return Ok(products.clone()),
            (ExtrasStep::Blocked, _) => {
                return Err(ApiError::Status {
                    status: 503,
                    body: format!("Princess extras ({currency}) failed recently; retrying later"),
                })
            }
            _ => {}
        }
        match fetch().await {
            Ok(v) => {
                let v = Arc::new(v);
                *entry = Some(ExtrasEntry::Ok { fetched_at: Instant::now(), products: v.clone() });
                Ok(v)
            }
            Err(e) => {
                *entry = Some(ExtrasEntry::Failed { retry_after: Instant::now() + EXTRAS_RETRY_AFTER });
                Err(e)
            }
        }
    }
}

/// Map a Princess product map to the shared fetch outcome. An empty catalog is
/// "nothing to record" (never a $0 price, never a failure).
pub fn princess_fetch_outcome(products: HashMap<String, PublicProduct>) -> PublicFetch {
    if products.is_empty() {
        PublicFetch::PlannerNotOpen
    } else {
        PublicFetch::Products(products.into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookings::login_outcome;
    use chrono::NaiveDate;

    fn prod(code: &str) -> PublicProduct {
        PublicProduct {
            category_id: "c".into(),
            category_name: "C".into(),
            product_code: code.into(),
            title: None,
            promo_cents: Some(1000),
            base_cents: None,
            promo_label: None,
            base_label: None,
            currency: Some("USD".into()),
            unit_label: None,
            child_cents: None,
        }
    }

    #[test]
    fn princess_empty_catalog_is_planner_not_open() {
        assert!(matches!(princess_fetch_outcome(HashMap::new()), PublicFetch::PlannerNotOpen));
    }

    #[test]
    fn princess_nonempty_is_products() {
        let mut m = HashMap::new();
        m.insert("2323".to_string(), prod("2323"));
        assert!(matches!(princess_fetch_outcome(m), PublicFetch::Products(v) if v.len() == 1));
    }

    #[test]
    fn merge_prefers_excursion_and_extras_by_code() {
        let m = merge_products(vec![prod("SJD-100")], vec![prod("2323")]);
        assert!(m.contains_key("SJD-100") && m.contains_key("2323"));
    }

    #[test]
    fn merge_collision_prefers_excursion() {
        let mut ex = prod("X");
        ex.promo_cents = Some(111);
        let mut xt = prod("X");
        xt.promo_cents = Some(222);
        let m = merge_products(vec![ex], vec![xt]);
        assert_eq!(m["X"].promo_cents, Some(111));
    }

    #[tokio::test]
    async fn failed_fetch_attempted_once_and_partial_returned() {
        use std::cell::Cell;
        let ex_calls = Cell::new(0);
        let xt_calls = Cell::new(0);
        let mut cache = PrincessPriceCache::default();
        for _ in 0..3 {
            let r = cache
                .products_with(
                    "V1",
                    "USD",
                    || async {
                        ex_calls.set(ex_calls.get() + 1);
                        Ok::<_, String>(vec![prod("SJD-1")])
                    },
                    || async {
                        xt_calls.set(xt_calls.get() + 1);
                        Err::<Vec<PublicProduct>, _>("boom".to_string())
                    },
                )
                .await
                .expect("excursions succeeded");
            assert!(r.contains_key("SJD-1") && r.len() == 1);
        }
        assert_eq!((ex_calls.get(), xt_calls.get()), (1, 1));

        // Both failing: None, and still only one attempt per key.
        let calls = Cell::new(0);
        let mut c2 = PrincessPriceCache::default();
        for _ in 0..3 {
            let r = c2
                .products_with(
                    "V2",
                    "USD",
                    || async {
                        calls.set(calls.get() + 1);
                        Err::<Vec<PublicProduct>, _>("x".to_string())
                    },
                    || async {
                        calls.set(calls.get() + 1);
                        Err::<Vec<PublicProduct>, _>("y".to_string())
                    },
                )
                .await;
            assert!(r.is_none());
        }
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn extras_step_ttl_and_failure_backoff() {
        let now = Instant::now() + Duration::from_secs(100 * 3600);
        let ok = |age: Duration| ExtrasEntry::Ok { fetched_at: now - age, products: Arc::new(vec![]) };
        assert_eq!(extras_step(None, now), ExtrasStep::Fetch);
        assert_eq!(extras_step(Some(&ok(Duration::from_secs(3600))), now), ExtrasStep::Use);
        assert_eq!(extras_step(Some(&ok(EXTRAS_TTL + Duration::from_secs(1))), now), ExtrasStep::Fetch);
        let failed = |retry_after| ExtrasEntry::Failed { retry_after };
        assert_eq!(extras_step(Some(&failed(now + Duration::from_secs(60))), now), ExtrasStep::Blocked);
        assert_eq!(extras_step(Some(&failed(now - Duration::from_secs(1))), now), ExtrasStep::Fetch);
    }

    #[tokio::test]
    async fn shared_extras_fetched_once_and_failure_not_refetched() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let cache = PrincessExtrasCache::default();
        for _ in 0..3 {
            let v = cache
                .get_with("USD", || async {
                    calls.set(calls.get() + 1);
                    Ok(vec![prod("2323")])
                })
                .await
                .unwrap();
            assert_eq!(v.len(), 1);
        }
        assert_eq!(calls.get(), 1);

        // A failure is remembered (no refetch inside the retry-after window),
        // and is per currency.
        let fails = Cell::new(0);
        for _ in 0..3 {
            let r = cache
                .get_with("AUD", || async {
                    fails.set(fails.get() + 1);
                    Err(ApiError::Status { status: 500, body: String::new() })
                })
                .await;
            assert!(r.is_err());
        }
        assert_eq!(fails.get(), 1);
        assert!(cache.get_with("USD", || async { unreachable!() }).await.is_ok());
    }

    fn ctx() -> PrincessBookingContext {
        PrincessBookingContext {
            booking_id: "AB12CD".into(),
            ship_code: "SKY".into(),
            voyage_code: "X637".into(),
            pax_id: "P1".into(),
            nights: Some(7),
            sail_date: NaiveDate::from_ymd_opt(2027, 3, 1).unwrap(),
            currency: "USD".into(),
        }
    }

    #[test]
    fn booking_from_context_maps_fields() {
        let b = booking_from_context(&ctx(), 7);
        assert_eq!(b.brand, Brand::Princess);
        assert_eq!(b.reservation_id, "AB12CD");
        assert_eq!(b.account_id, "princess:7");
        assert_eq!(b.voyage_code.as_deref(), Some("X637"));
        assert_eq!(b.currency.as_deref(), Some("USD"));
        assert_eq!(b.passenger_id.as_deref(), Some("P1"));
        assert!(b.stateroom.is_none() && b.assigned_stateroom.is_none() && b.package_code.is_none());
    }

    #[test]
    fn princess_discovery_no_booking_records_error() {
        let r = DiscoveryReport { persisted: 0, logins_ok: 1, errors: vec![PRINCESS_NO_BOOKING_MSG.into()] };
        assert_eq!(login_outcome(&r).as_deref(), Some(PRINCESS_NO_BOOKING_MSG));
    }
}
