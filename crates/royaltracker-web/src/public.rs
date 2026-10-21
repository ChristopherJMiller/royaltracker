//! Public (no-login) tier: look up any sailing's Cruise Planner prices with just
//! a ship + sail date. No Telegram initData, no account, no reservation number.
//!
//! On a cache miss/staleness, `/public/prices` fetches the public catalog live
//! (appkey-only), records a snapshot, and SEEDS a tracked_product so the daily
//! sweep keeps it fresh — so the lookup shows data immediately and bootstraps
//! ongoing tracking.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{Duration, NaiveDate, Utc};
use royaltracker_api::princess::{PrincessGuestClient, PrincessVoyage};
use royaltracker_api::{PublicClient, PublicFetch};
use royaltracker_storage::{
    DefaultRepo, NewPublicChannel, PriceRepo, PublicPriceDto, SailingKey, SailingSnapshot,
};
use royaltracker_types::{AlertMode, Brand, PublicChannelKind};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;

use crate::device;

/// How long a cached snapshot is considered fresh before a lookup re-fetches.
const FRESH_HOURS: i64 = 12;

/// Identity/anti-abuse config for subscribe + manage. Absent → those routes are
/// disabled (the read-only lookup still works with none of this).
#[derive(Clone)]
pub struct PublicIdentity {
    /// Vanilla TLS client for Turnstile siteverify (NOT the wreq scraping stack).
    pub http: reqwest::Client,
    pub turnstile_secret: String,
    pub turnstile_site_key: String,
    pub device_cookie_key: Arc<Vec<u8>>,
    /// Served to the browser so it can subscribe to web push (identity phase).
    pub vapid_public_key: Option<String>,
}

impl PublicIdentity {
    /// Build from config strings. Returns None (subscribe stays disabled) if the
    /// device-cookie key isn't valid base64 of at least 16 bytes.
    pub fn from_parts(
        turnstile_secret: String,
        turnstile_site_key: String,
        device_cookie_key_b64: &str,
        vapid_public_key: Option<String>,
    ) -> Option<Self> {
        use base64::Engine;
        let key = base64::engine::general_purpose::STANDARD
            .decode(device_cookie_key_b64.trim())
            .ok()?;
        if key.len() < 16 {
            return None;
        }
        Some(Self {
            http: reqwest::Client::new(),
            turnstile_secret,
            turnstile_site_key,
            device_cookie_key: Arc::new(key),
            vapid_public_key,
        })
    }
}

/// Cached Princess schedule with the time it was fetched.
pub type ScheduleCache = Arc<tokio::sync::RwLock<Option<(std::time::Instant, Vec<PrincessVoyage>)>>>;

const SCHEDULE_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

#[derive(Clone)]
pub struct PublicState {
    pub repo: Arc<DefaultRepo>,
    /// appkey-only client for live seed-on-lookup fetches. Egresses from the
    /// pod's (home) IP, never Cloudflare.
    pub public_client: Arc<PublicClient>,
    /// Princess guest client (schedule source).
    pub princess: Arc<PrincessGuestClient>,
    /// Whole Princess schedule, refreshed at most every 24 h.
    pub schedule: ScheduleCache,
    /// None → subscribe/manage disabled (read-only lookup still works).
    pub identity: Option<PublicIdentity>,
}

pub fn public_router(state: PublicState) -> Router {
    Router::new()
        .route("/public/ships", get(ships))
        .route("/public/sailings", get(sailings))
        .route("/public/prices", get(prices))
        .route("/public/config", get(config))
        .route("/public/subscribe", post(subscribe))
        .route("/public/subscriptions", get(list_subscriptions))
        .route("/public/subscriptions/:id", delete(remove_subscription))
        .route("/p", get(shell))
        .route("/p/", get(shell))
        .route("/p/sw.js", get(service_worker))
        .with_state(state)
}

type ApiErr = (StatusCode, String);

fn parse_brand(s: &str) -> Result<Brand, ApiErr> {
    Brand::from_str(s).map_err(|_| (StatusCode::BAD_REQUEST, "unknown brand".into()))
}

fn parse_date(s: &str) -> Result<NaiveDate, ApiErr> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| (StatusCode::BAD_REQUEST, "bad sail_date (want YYYY-MM-DD)".into()))
}

fn db_err(e: impl std::fmt::Display) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("db: {e}"))
}

/// Princess sailings are keyed by voyage code (ship + date isn't unique).
fn require_voyage(brand: Brand, voyage: Option<&str>) -> Result<Option<&str>, ApiErr> {
    let voyage = voyage.map(str::trim).filter(|v| !v.is_empty());
    if brand == Brand::Princess && voyage.is_none() {
        return Err((StatusCode::BAD_REQUEST, "voyage required for princess".into()));
    }
    Ok(if brand == Brand::Princess { voyage } else { None })
}

#[derive(Serialize)]
struct ShipItem {
    brand: String,
    ship_code: String,
    ship_name: String,
}

/// The full static ship catalog so the picker always works, even on a fresh
/// deployment with nothing tracked yet.
async fn ships() -> Json<Vec<ShipItem>> {
    let items = royaltracker_types::all_ships()
        .iter()
        .map(|(code, name, brand)| ShipItem {
            brand: brand.as_str().to_string(),
            ship_code: code.to_string(),
            ship_name: name.to_string(),
        })
        .collect();
    Json(items)
}

#[derive(Deserialize)]
struct SailingsQuery {
    brand: String,
    ship: String,
}



#[derive(Serialize, Debug, PartialEq, Eq)]
struct SailingOption {
    sail_date: String,
    voyage_code: Option<String>,
    nights: Option<i32>,
    itinerary: Option<String>,
}

/// Current schedule, refetched when absent or older than 24 h. A failed refresh
/// serves the stale copy if there is one.
async fn princess_schedule(s: &PublicState) -> Result<Vec<PrincessVoyage>, ApiErr> {
    {
        let g = s.schedule.read().await;
        if let Some((at, v)) = g.as_ref() {
            if at.elapsed() < SCHEDULE_TTL {
                return Ok(v.clone());
            }
        }
    }
    let mut g = s.schedule.write().await;
    // Another request may have refreshed while we waited for the write lock.
    if let Some((at, v)) = g.as_ref() {
        if at.elapsed() < SCHEDULE_TTL {
            return Ok(v.clone());
        }
    }
    match s.princess.fetch_schedule().await {
        Ok(v) => {
            *g = Some((std::time::Instant::now(), v.clone()));
            Ok(v)
        }
        Err(e) => match g.as_ref() {
            Some((_, v)) => {
                tracing::warn!(error = %e, "princess schedule refresh failed; serving stale");
                Ok(v.clone())
            }
            None => Err((StatusCode::BAD_GATEWAY, format!("schedule: {e}"))),
        },
    }
}

/// Upcoming voyages of one Princess ship from a schedule (already sorted).
fn princess_options(schedule: &[PrincessVoyage], ship: &str, today: NaiveDate) -> Vec<SailingOption> {
    schedule
        .iter()
        .filter(|v| v.ship_code == ship && v.sail_date >= today)
        .map(|v| SailingOption {
            sail_date: v.sail_date.format("%Y-%m-%d").to_string(),
            voyage_code: Some(v.voyage_code.clone()),
            nights: Some(v.nights),
            itinerary: Some(v.itinerary.clone()),
        })
        .collect()
}

/// Bookable sailings for a ship. Royal/Celebrity: dates from the public voyages
/// endpoint. Princess: every voyage (ship + date isn't unique) from the cached
/// public schedule.
async fn sailings(
    State(s): State<PublicState>,
    Query(q): Query<SailingsQuery>,
) -> Result<Json<Vec<SailingOption>>, ApiErr> {
    let brand = parse_brand(&q.brand)?;
    if brand == Brand::Princess {
        let sched = princess_schedule(&s).await?;
        return Ok(Json(princess_options(&sched, &q.ship, Utc::now().date_naive())));
    }
    let dates = s
        .public_client
        .fetch_public_sailings(brand, &q.ship)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("voyages: {e}")))?;
    Ok(Json(
        dates
            .iter()
            .map(|d| SailingOption {
                sail_date: d.format("%Y-%m-%d").to_string(),
                voyage_code: None,
                nights: None,
                itinerary: None,
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(code: &str, ship: &str, d: (i32, u32, u32)) -> PrincessVoyage {
        PrincessVoyage {
            voyage_code: code.into(),
            ship_code: ship.into(),
            sail_date: NaiveDate::from_ymd_opt(d.0, d.1, d.2).unwrap(),
            nights: 7,
            itinerary: "Caribbean".into(),
            currency: "USD".into(),
        }
    }

    #[test]
    fn princess_prices_require_voyage() {
        assert!(require_voyage(Brand::Princess, None).is_err());
        assert!(require_voyage(Brand::Royal, None).is_ok());
        assert_eq!(require_voyage(Brand::Princess, Some("4702A")).unwrap(), Some("4702A"));
    }

    #[test]
    fn princess_options_filter_ship_and_past_keep_combos() {
        let s = vec![
            v("A1", "ST", (2020, 1, 1)),
            v("4702", "ST", (2027, 1, 2)),
            v("4702A", "ST", (2027, 1, 2)),
            v("X1", "RU", (2027, 1, 2)),
        ];
        let today = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
        let o = princess_options(&s, "ST", today);
        assert_eq!(o.len(), 2);
        assert_eq!(o[0].voyage_code.as_deref(), Some("4702"));
        assert_eq!(o[1].voyage_code.as_deref(), Some("4702A"));
        assert_eq!(o[0].sail_date, "2027-01-02");
        assert_eq!(o[0].nights, Some(7));
    }
}

#[derive(Deserialize)]
struct PricesQuery {
    brand: String,
    ship: String,
    sail_date: String,
    /// Required for Princess (voyage code); ignored for Royal/Celebrity.
    #[serde(default)]
    voyage: Option<String>,
}

#[derive(Serialize)]
struct PricesResponse {
    planner_open: bool,
    live: bool,
    prices: Vec<PublicPriceDto>,
}

/// The headline endpoint: current public promo prices for a sailing. Serves
/// cached data when fresh, otherwise fetches live + seeds tracking.
async fn prices(
    State(s): State<PublicState>,
    Query(q): Query<PricesQuery>,
) -> Result<Json<PricesResponse>, ApiErr> {
    let brand = parse_brand(&q.brand)?;
    let date = parse_date(&q.sail_date)?;
    let voyage = require_voyage(brand, q.voyage.as_deref())?;
    let key = SailingKey { brand, ship_code: &q.ship, sail_date: date, voyage_code: voyage };

    let cached = s.repo.latest_sailing_prices(&key).await.map_err(db_err)?;
    let cutoff = Utc::now() - Duration::hours(FRESH_HOURS);
    let is_fresh = !cached.is_empty()
        && cached
            .iter()
            .any(|p| p.fetched_at.map(|t| t >= cutoff).unwrap_or(false));
    if is_fresh {
        return Ok(Json(PricesResponse {
            planner_open: true,
            live: false,
            prices: cached,
        }));
    }

    // Cache miss or stale → live fetch + seed.
    let (fetch, currency) = match voyage {
        Some(v) => {
            let currency = princess_schedule(&s)
                .await?
                .iter()
                .find(|x| x.voyage_code == v)
                .map(|x| x.currency.clone())
                .unwrap_or_else(|| "USD".to_string());
            let mut cache = royaltracker_core::PrincessPriceCache::default();
            let r = cache
                .products(&s.princess, v, &currency)
                .await
                .map(royaltracker_core::princess_fetch_outcome);
            (r, Some(currency))
        }
        None => (s.public_client.fetch_public_products(brand, &q.ship, date).await, None),
    };
    match fetch {
        Ok(PublicFetch::Products(products)) => {
            let sailing_id = s.repo.upsert_sailing(&key, currency.as_deref()).await.map_err(db_err)?;
            for p in &products {
                let tracked_id = s
                    .repo
                    .upsert_tracked_product(
                        sailing_id,
                        &p.product_code,
                        &p.category_id,
                        p.title.as_deref(),
                        None,
                    )
                    .await
                    .map_err(db_err)?;
                let snap = SailingSnapshot {
                    tracked_id,
                    fetched_at: Utc::now(),
                    adult_promo_price: p.promo_dollars(),
                    child_promo_price: p.child_cents.map(|c| c as f64 / 100.0),
                    base_price: p.base_dollars(),
                    promo_present: p.promo_cents.is_some(),
                    raw_response: serde_json::to_value(p).unwrap_or(serde_json::Value::Null),
                };
                if let Err(e) = s.repo.record_public_price(tracked_id, &snap).await {
                    tracing::warn!(error = %e, "record_public_price (lookup seed) failed");
                }
            }
            let fresh = s.repo.latest_sailing_prices(&key).await.map_err(db_err)?;
            Ok(Json(PricesResponse {
                planner_open: true,
                live: true,
                prices: fresh,
            }))
        }
        Ok(PublicFetch::PlannerNotOpen) => Ok(Json(PricesResponse {
            planner_open: false,
            live: true,
            prices: cached,
        })),
        Err(e) => Err((StatusCode::BAD_GATEWAY, format!("fetch: {e}"))),
    }
}

#[derive(Serialize)]
struct PublicConfigResp {
    subscribe_enabled: bool,
    turnstile_site_key: Option<String>,
    vapid_public_key: Option<String>,
}

/// Frontend bootstrap: whether subscribe is available + the public keys it needs.
async fn config(State(s): State<PublicState>) -> Json<PublicConfigResp> {
    match &s.identity {
        Some(id) => Json(PublicConfigResp {
            subscribe_enabled: true,
            turnstile_site_key: Some(id.turnstile_site_key.clone()),
            vapid_public_key: id.vapid_public_key.clone(),
        }),
        None => Json(PublicConfigResp {
            subscribe_enabled: false,
            turnstile_site_key: None,
            vapid_public_key: None,
        }),
    }
}

#[derive(Deserialize)]
struct PushReg {
    endpoint: String,
    p256dh: String,
    auth: String,
}

#[derive(Deserialize)]
struct SubscribeReq {
    brand: String,
    ship: String,
    sail_date: String,
    product_code: String,
    /// Required for Princess (voyage code).
    #[serde(default)]
    voyage: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    label: Option<String>,
    /// "email" | "webpush"
    channel: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    push: Option<PushReg>,
    #[serde(default)]
    alert_mode: Option<String>,
    #[serde(default)]
    threshold: Option<f64>,
    #[serde(default)]
    turnstile_token: Option<String>,
}

#[derive(Serialize)]
struct SubscribeResp {
    ok: bool,
    subscription_id: i64,
}

/// Create a public subscription. Turnstile-gated, mints/reuses a device cookie.
async fn subscribe(
    State(s): State<PublicState>,
    headers: HeaderMap,
    Json(body): Json<SubscribeReq>,
) -> Result<Response, ApiErr> {
    let Some(identity) = s.identity.as_ref() else {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "subscribe not enabled".into()));
    };

    // Anti-abuse: verify Turnstile server-side (fail closed).
    let token = body
        .turnstile_token
        .as_deref()
        .ok_or((StatusCode::BAD_REQUEST, "missing turnstile token".into()))?;
    if !device::verify_turnstile(&identity.http, &identity.turnstile_secret, token, None).await {
        return Err((StatusCode::FORBIDDEN, "turnstile verification failed".into()));
    }

    let brand = parse_brand(&body.brand)?;
    let date = parse_date(&body.sail_date)?;
    let voyage = require_voyage(brand, body.voyage.as_deref())?;
    let kind = match body.channel.as_str() {
        "webpush" => PublicChannelKind::WebPush,
        "email" => {
            return Err((
                StatusCode::BAD_REQUEST,
                "email isn't supported — use browser push or the Telegram bot".into(),
            ))
        }
        _ => return Err((StatusCode::BAD_REQUEST, "channel must be webpush".into())),
    };
    let alert_mode = body
        .alert_mode
        .as_deref()
        .map(AlertMode::from_str)
        .transpose()
        .map_err(|_| (StatusCode::BAD_REQUEST, "bad alert_mode".into()))?
        .unwrap_or(AlertMode::AnyDrop);

    // Resolve the channel's endpoint + keys.
    let (endpoint, p256dh, auth): (String, Option<String>, Option<String>) = match kind {
        PublicChannelKind::Email => {
            let e = body
                .email
                .clone()
                .filter(|e| e.contains('@'))
                .ok_or((StatusCode::BAD_REQUEST, "valid email required".into()))?;
            (e, None, None)
        }
        PublicChannelKind::WebPush => {
            let p = body
                .push
                .as_ref()
                .ok_or((StatusCode::BAD_REQUEST, "push subscription required".into()))?;
            (p.endpoint.clone(), Some(p.p256dh.clone()), Some(p.auth.clone()))
        }
    };

    // Device cookie: reuse if present+valid, else mint.
    let key = identity.device_cookie_key.as_slice();
    let (device_id, set_cookie) = match device::read_device(&headers, key) {
        Some(id) => (id, false),
        None => (device::mint_device_id(), true),
    };

    // Seed the sailing + tracked product so the sweep keeps it fresh.
    let currency = match voyage {
        Some(v) => Some(
            princess_schedule(&s)
                .await?
                .iter()
                .find(|x| x.voyage_code == v)
                .map(|x| x.currency.clone())
                .unwrap_or_else(|| "USD".to_string()),
        ),
        None => None,
    };
    let sailing_id = s
        .repo
        .upsert_sailing(
            &SailingKey { brand, ship_code: &body.ship, sail_date: date, voyage_code: voyage },
            currency.as_deref(),
        )
        .await
        .map_err(db_err)?;
    let category = body.category.clone().unwrap_or_default();
    let tracked_id = s
        .repo
        .upsert_tracked_product(
            sailing_id,
            &body.product_code,
            &category,
            body.label.as_deref(),
            None,
        )
        .await
        .map_err(db_err)?;

    let channel_id = s
        .repo
        .upsert_public_channel(&NewPublicChannel {
            kind,
            endpoint: &endpoint,
            p256dh: p256dh.as_deref(),
            auth: auth.as_deref(),
            device_token: Some(&device_id),
            // Web push is usable immediately; email must be verified first.
            verified: matches!(kind, PublicChannelKind::WebPush),
        })
        .await
        .map_err(db_err)?;

    let subscription_id = s
        .repo
        .subscribe_public(channel_id, tracked_id, alert_mode, body.threshold)
        .await
        .map_err(db_err)?;

    let json = serde_json::to_string(&SubscribeResp {
        ok: true,
        subscription_id,
    })
    .unwrap_or_default();
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json");
    if set_cookie {
        builder = builder.header(header::SET_COOKIE, device::set_cookie_header(&device_id, key));
    }
    Ok(builder.body(axum::body::Body::from(json)).unwrap())
}

async fn list_subscriptions(
    State(s): State<PublicState>,
    headers: HeaderMap,
) -> Result<Response, ApiErr> {
    let device_id = device_from(&s, &headers)?;
    let rows = s
        .repo
        .list_public_subscriptions_for_device(&device_id)
        .await
        .map_err(db_err)?;
    Ok(Json(rows).into_response())
}

#[derive(Serialize)]
struct RemoveResp {
    removed: bool,
}

async fn remove_subscription(
    State(s): State<PublicState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<RemoveResp>, ApiErr> {
    let device_id = device_from(&s, &headers)?;
    let removed = s
        .repo
        .deactivate_public_subscription(id, &device_id)
        .await
        .map_err(db_err)?;
    Ok(Json(RemoveResp { removed }))
}

/// Resolve the verified device id, or 401 if there is no valid device cookie.
fn device_from(s: &PublicState, headers: &HeaderMap) -> Result<String, ApiErr> {
    let identity = s
        .identity
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "not enabled".into()))?;
    device::read_device(headers, identity.device_cookie_key.as_slice())
        .ok_or((StatusCode::UNAUTHORIZED, "no device".into()))
}

async fn shell() -> Html<&'static str> {
    Html(include_str!("../static/public/index.html"))
}

async fn service_worker() -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        include_str!("../static/public/sw.js"),
    )
        .into_response()
}
