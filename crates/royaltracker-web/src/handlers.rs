use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use royaltracker_storage::{CatalogEntry, HistoryPoint, PriceRepo};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::{AuthedUser, TgAuthOnly};
use crate::state::AppState;
use royaltracker_api::{CruiseClient, CruiseClientConfig};
use royaltracker_storage::NewUser;
use royaltracker_types::Brand;

type ApiError = (StatusCode, String);

fn db_err(e: impl std::fmt::Display) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

#[derive(Deserialize)]
pub struct RegisterBody {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct RegisterResponse {
    pub login_ok: bool,
    pub bookings_discovered: usize,
    pub message: String,
}

pub async fn register(
    State(s): State<Arc<AppState>>,
    auth: TgAuthOnly,
    Json(body): Json<RegisterBody>,
) -> Result<Json<RegisterResponse>, ApiError> {
    let email = body.email.trim().to_string();
    if !email.contains('@') {
        return Err((StatusCode::BAD_REQUEST, "invalid email".into()));
    }
    if body.password.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "password is empty".into()));
    }

    let (nonce, ct) = s
        .cipher
        .encrypt(body.password.as_bytes())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("encryption: {e}")))?;

    s.repo
        .upsert_user(&NewUser {
            telegram_chat_id: auth.tg.id,
            telegram_username: auth.tg.username.as_deref(),
            rcg_username: &email,
            rcg_password_ct: &ct,
            rcg_password_nonce: &nonce,
            // brand_pref is legacy: scraper now hits both brands regardless.
            // Default to Royal; the value is unused by the discovery flow.
            brand_pref: Brand::Royal,
        })
        .await
        .map_err(db_err)?;

    let db_user = s
        .repo
        .get_user_by_chat_id(auth.tg.id)
        .await
        .map_err(db_err)?
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "user vanished post-upsert".into()))?;

    let report = royaltracker_core::discover_and_persist_bookings(
        &email,
        &body.password,
        s.rcg_basic_auth_b64.as_ref(),
        s.repo.as_ref(),
        &db_user,
    )
    .await
    .map_err(|e| (StatusCode::BAD_GATEWAY, format!("discovery: {e}")))?;

    let login_ok = report.logins_ok > 0;
    let message = if login_ok && report.errors.is_empty() {
        format!(
            "Linked. Discovered {n} booking{plural} across {brands} brand{bplural}.",
            n = report.persisted,
            plural = if report.persisted == 1 { "" } else { "s" },
            brands = report.logins_ok,
            bplural = if report.logins_ok == 1 { "" } else { "s" },
        )
    } else if login_ok {
        format!(
            "Linked partially. {n} bookings persisted; some brands errored: {errs}",
            n = report.persisted,
            errs = report.errors.join("; "),
        )
    } else {
        format!(
            "Credentials saved, but no brand login succeeded: {errs}. \
             Re-register to overwrite, or wait if rate-limited.",
            errs = report.errors.join("; "),
        )
    };

    Ok(Json(RegisterResponse {
        login_ok,
        bookings_discovered: report.persisted,
        message,
    }))
}

pub async fn refresh(
    State(s): State<Arc<AppState>>,
    user: AuthedUser,
) -> Result<Json<RefreshResponse>, ApiError> {
    let pw_bytes = s
        .cipher
        .decrypt(&user.db_user.rcg_password_nonce, &user.db_user.rcg_password_ct)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("decrypt: {e}")))?;
    let password = String::from_utf8(pw_bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("password not utf8: {e}")))?;

    let report = royaltracker_core::discover_and_persist_bookings(
        &user.db_user.rcg_username,
        &password,
        s.rcg_basic_auth_b64.as_ref(),
        s.repo.as_ref(),
        &user.db_user,
    )
    .await
    .map_err(|e| (StatusCode::BAD_GATEWAY, format!("refresh: {e}")))?;

    Ok(Json(RefreshResponse {
        bookings_discovered: report.persisted,
        errors: report.errors,
    }))
}

#[derive(Serialize)]
pub struct RefreshResponse {
    pub bookings_discovered: usize,
    pub errors: Vec<String>,
}

#[derive(Deserialize)]
pub struct CatalogRefreshBody {
    pub reservation_id: String,
}

#[derive(Serialize)]
pub struct CatalogRefreshResponse {
    pub products_persisted: usize,
}

pub async fn refresh_catalog(
    State(s): State<Arc<AppState>>,
    user: AuthedUser,
    Json(body): Json<CatalogRefreshBody>,
) -> Result<Json<CatalogRefreshResponse>, ApiError> {
    let all = s.repo.list_bookings().await.map_err(db_err)?;
    let booking = all
        .into_iter()
        .find(|b| {
            b.reservation_id == body.reservation_id && b.user_id == Some(user.db_user.id)
        })
        .ok_or((StatusCode::NOT_FOUND, "booking not found".into()))?;

    // GraphQL endpoint is anonymous (appkey only), so we don't need the user's
    // credentials at all. Build a client with a placeholder brand.
    let api_cfg = CruiseClientConfig::web(
        royaltracker_types::Brand::Royal,
        String::new(),
        String::new(),
        s.rcg_basic_auth_b64.as_ref().clone(),
    );
    let client = CruiseClient::new(api_cfg)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("client: {e}")))?;

    let persisted = royaltracker_core::refresh_catalog_for_booking(&client, s.repo.as_ref(), &booking)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("catalog: {e}")))?;

    Ok(Json(CatalogRefreshResponse {
        products_persisted: persisted,
    }))
}

#[derive(Serialize)]
pub struct MeResponse {
    pub tg_id: i64,
    pub tg_username: Option<String>,
    pub tg_first_name: Option<String>,
    pub rcg_username: String,
    pub brand: String,
}

pub async fn get_me(user: AuthedUser) -> Json<MeResponse> {
    Json(MeResponse {
        tg_id: user.tg.id,
        tg_username: user.tg.username,
        tg_first_name: user.tg.first_name,
        rcg_username: user.db_user.rcg_username,
        brand: user.db_user.brand_pref.to_string(),
    })
}

#[derive(Serialize)]
pub struct BookingDto {
    pub reservation_id: String,
    pub brand: String,
    pub ship_code: String,
    pub sail_date: chrono::NaiveDate,
    pub nights: Option<i32>,
    pub package_code: Option<String>,
}

pub async fn list_bookings(
    State(s): State<Arc<AppState>>,
    user: AuthedUser,
) -> Result<Json<Vec<BookingDto>>, ApiError> {
    let all = s.repo.list_bookings().await.map_err(db_err)?;
    let mine = all
        .into_iter()
        .filter(|b| b.user_id == Some(user.db_user.id))
        .map(|b| BookingDto {
            reservation_id: b.reservation_id,
            brand: b.brand.to_string(),
            ship_code: b.ship_code,
            sail_date: b.sail_date,
            nights: b.nights,
            package_code: b.package_code,
        })
        .collect();
    Ok(Json(mine))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    10
}

pub async fn search_catalog(
    State(s): State<Arc<AppState>>,
    _user: AuthedUser,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<CatalogDto>>, ApiError> {
    let entries = s
        .repo
        .search_catalog(&q.q, q.limit)
        .await
        .map_err(db_err)?;
    Ok(Json(entries.into_iter().map(CatalogDto::from).collect()))
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    pub reservation_id: String,
}

pub async fn browse_catalog(
    State(s): State<Arc<AppState>>,
    _user: AuthedUser,
    Query(q): Query<BrowseQuery>,
) -> Result<Json<Vec<CatalogDto>>, ApiError> {
    let entries = s
        .repo
        .list_catalog_by_reservation(&q.reservation_id)
        .await
        .map_err(db_err)?;
    Ok(Json(entries.into_iter().map(CatalogDto::from).collect()))
}

#[derive(Serialize)]
pub struct CatalogDto {
    pub reservation_id: String,
    pub category_id: String,
    pub category_name: String,
    pub product_code: String,
    pub title: String,
    pub summary: Option<String>,
    pub starting_price: Option<f64>,
    pub currency: Option<String>,
    pub price_label: Option<String>,
    pub base_price_label: Option<String>,
    pub unit_label: Option<String>,
}

impl From<CatalogEntry> for CatalogDto {
    fn from(e: CatalogEntry) -> Self {
        Self {
            reservation_id: e.reservation_id,
            category_id: e.category_id,
            category_name: e.category_name,
            product_code: e.product_code,
            title: e.title,
            summary: e.summary,
            starting_price: e.starting_price,
            currency: e.currency,
            price_label: e.price_label,
            base_price_label: e.base_price_label,
            unit_label: e.unit_label,
        }
    }
}

#[derive(Serialize)]
pub struct WatchedDto {
    pub id: i64,
    pub reservation_id: String,
    pub category_prefix: String,
    pub product_code: String,
    pub label: Option<String>,
}

pub async fn list_watched(
    State(s): State<Arc<AppState>>,
    _user: AuthedUser,
) -> Result<Json<Vec<WatchedDto>>, ApiError> {
    let ws = s.repo.list_active_watched().await.map_err(db_err)?;
    Ok(Json(
        ws.into_iter()
            .map(|w| WatchedDto {
                id: w.id,
                reservation_id: w.reservation_id,
                category_prefix: w.category_prefix,
                product_code: w.product_code,
                label: w.label,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct AddWatchedBody {
    pub reservation_id: String,
    pub category_prefix: String,
    pub product_code: String,
    pub label: Option<String>,
}

pub async fn add_watched(
    State(s): State<Arc<AppState>>,
    _user: AuthedUser,
    Json(body): Json<AddWatchedBody>,
) -> Result<Json<i64>, ApiError> {
    let id = s
        .repo
        .upsert_watched(
            &body.reservation_id,
            &body.category_prefix,
            &body.product_code,
            body.label.as_deref(),
        )
        .await
        .map_err(db_err)?;
    Ok(Json(id))
}

pub async fn remove_watched(
    State(_s): State<Arc<AppState>>,
    _user: AuthedUser,
    Path(_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    // For now soft-removal is via active flag; storage layer doesn't expose that yet.
    // Adding a proper deactivate_watched method is a small follow-up.
    Err((
        StatusCode::NOT_IMPLEMENTED,
        "remove_watched: pending storage method".into(),
    ))
}

pub async fn history(
    State(s): State<Arc<AppState>>,
    _user: AuthedUser,
    Path(watched_id): Path<i64>,
) -> Result<Json<Vec<HistoryPoint>>, ApiError> {
    let points = s
        .repo
        .snapshot_history(watched_id, 365)
        .await
        .map_err(db_err)?;
    Ok(Json(points))
}
