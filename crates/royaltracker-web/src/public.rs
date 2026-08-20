//! Public (no-login) tier: look up any sailing's Cruise Planner prices with just
//! a ship + sail date. No Telegram initData, no account, no reservation number.
//!
//! On a cache miss/staleness, `/public/prices` fetches the public catalog live
//! (appkey-only), records a snapshot, and SEEDS a tracked_product so the daily
//! sweep keeps it fresh — so the lookup shows data immediately and bootstraps
//! ongoing tracking.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{Duration, NaiveDate, Utc};
use royaltracker_api::{PublicClient, PublicFetch};
use royaltracker_storage::{DefaultRepo, PriceRepo, PublicPriceDto, SailingSnapshot};
use royaltracker_types::Brand;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;

/// How long a cached snapshot is considered fresh before a lookup re-fetches.
const FRESH_HOURS: i64 = 12;

#[derive(Clone)]
pub struct PublicState {
    pub repo: Arc<DefaultRepo>,
    /// appkey-only client for live seed-on-lookup fetches. Egresses from the
    /// pod's (home) IP, never Cloudflare.
    pub public_client: Arc<PublicClient>,
}

pub fn public_router(state: PublicState) -> Router {
    Router::new()
        .route("/public/ships", get(ships))
        .route("/public/sailings", get(sailings))
        .route("/public/prices", get(prices))
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

/// Bookable sail dates for a ship, fetched live from the public voyages endpoint.
async fn sailings(
    State(s): State<PublicState>,
    Query(q): Query<SailingsQuery>,
) -> Result<Json<Vec<String>>, ApiErr> {
    let brand = parse_brand(&q.brand)?;
    let dates = s
        .public_client
        .fetch_public_sailings(brand, &q.ship)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("voyages: {e}")))?;
    Ok(Json(
        dates.iter().map(|d| d.format("%Y-%m-%d").to_string()).collect(),
    ))
}

#[derive(Deserialize)]
struct PricesQuery {
    brand: String,
    ship: String,
    sail_date: String,
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

    let cached = s
        .repo
        .latest_sailing_prices(brand, &q.ship, date)
        .await
        .map_err(db_err)?;
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
    match s
        .public_client
        .fetch_public_products(brand, &q.ship, date)
        .await
    {
        Ok(PublicFetch::Products(products)) => {
            let sailing_id = s.repo.upsert_sailing(brand, &q.ship, date).await.map_err(db_err)?;
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
                    child_promo_price: None,
                    base_price: p.base_dollars(),
                    promo_present: p.promo_cents.is_some(),
                    raw_response: serde_json::to_value(p).unwrap_or(serde_json::Value::Null),
                };
                if let Err(e) = s.repo.record_public_price(tracked_id, &snap).await {
                    tracing::warn!(error = %e, "record_public_price (lookup seed) failed");
                }
            }
            let fresh = s
                .repo
                .latest_sailing_prices(brand, &q.ship, date)
                .await
                .map_err(db_err)?;
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
