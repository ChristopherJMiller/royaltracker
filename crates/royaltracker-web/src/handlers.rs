use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use royaltracker_storage::{CatalogEntry, DeckPlan, HistoryPoint, PriceRepo};
use royaltracker_types::AlertMode;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::{AuthedUser, TgAuthOnly};
use crate::state::AppState;
use royaltracker_api::{CruiseClient, CruiseClientConfig};
use royaltracker_storage::{NewCruiseAccount, NewUser};
use royaltracker_types::{Brand, CruiseAccount, CruiseLine};

type ApiError = (StatusCode, String);

fn db_err(e: impl std::fmt::Display) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

#[derive(Deserialize)]
pub struct RegisterBody {
    #[serde(default = "default_line")]
    pub cruise_line: CruiseLine,
    #[serde(alias = "email")]
    pub login: String,
    pub password: String,
}

fn default_line() -> CruiseLine {
    CruiseLine::Rcg
}

#[derive(Serialize)]
pub struct RegisterResponse {
    pub login_ok: bool,
    pub bookings_discovered: usize,
    pub message: String,
}

fn validate_register(line: CruiseLine, login: &str, password: &str) -> Result<(), ApiError> {
    if password.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "password is empty".into()));
    }
    if login.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "login is empty".into()));
    }
    if line == CruiseLine::Rcg && !login.contains('@') {
        return Err((StatusCode::BAD_REQUEST, "invalid email".into()));
    }
    Ok(())
}

fn decrypt_password(s: &AppState, acct: &CruiseAccount) -> Result<String, ApiError> {
    let pw_bytes = s
        .cipher
        .decrypt(&acct.password_nonce, &acct.password_ct)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("decrypt: {e}")))?;
    String::from_utf8(pw_bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("password not utf8: {e}")))
}

#[derive(Serialize)]
pub struct AccountDto {
    pub cruise_line: String,
    pub label: String,
    pub login: String,
    pub active: bool,
    pub last_login_at: Option<chrono::DateTime<Utc>>,
    pub last_login_error: Option<String>,
}

impl From<&CruiseAccount> for AccountDto {
    fn from(a: &CruiseAccount) -> Self {
        AccountDto {
            cruise_line: a.line.as_str().to_string(),
            label: a.line.label().to_string(),
            login: a.login.clone(),
            active: a.active,
            last_login_at: a.last_login_at,
            last_login_error: a.last_login_error.clone(),
        }
    }
}

pub async fn register(
    State(s): State<Arc<AppState>>,
    auth: TgAuthOnly,
    Json(body): Json<RegisterBody>,
) -> Result<Json<RegisterResponse>, ApiError> {
    let login = body.login.trim().to_string();
    validate_register(body.cruise_line, &login, &body.password)?;

    let (nonce, ct) = s
        .cipher
        .encrypt(body.password.as_bytes())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("encryption: {e}")))?;

    let user_id = s
        .repo
        .upsert_user(&NewUser {
            telegram_chat_id: auth.tg.id,
            telegram_username: auth.tg.username.as_deref(),
        })
        .await
        .map_err(db_err)?;
    s.repo
        .upsert_cruise_account(&NewCruiseAccount {
            user_id,
            line: body.cruise_line,
            login: &login,
            password_ct: &ct,
            password_nonce: &nonce,
        })
        .await
        .map_err(db_err)?;

    let db_user = s
        .repo
        .get_user_by_chat_id(auth.tg.id)
        .await
        .map_err(db_err)?
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "user vanished post-upsert".into()))?;
    let account = s
        .repo
        .list_cruise_accounts(user_id)
        .await
        .map_err(db_err)?
        .into_iter()
        .find(|a| a.line == body.cruise_line)
        .ok_or((StatusCode::INTERNAL_SERVER_ERROR, "account vanished post-upsert".into()))?;

    let report = royaltracker_core::discover_account(
        s.repo.as_ref(),
        s.rcg_basic_auth_b64.as_ref(),
        &db_user,
        &account,
        &body.password,
        None,
    )
    .await
    .map_err(|e| (StatusCode::BAD_GATEWAY, format!("discovery: {e}")))?
    .report;

    let login_ok = report.logins_ok > 0;
    let message = register_message(body.cruise_line, report.persisted, report.logins_ok, &report.errors);

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
    let accounts = s
        .repo
        .list_cruise_accounts(user.db_user.id)
        .await
        .map_err(db_err)?;
    let mut total = 0usize;
    let mut errors = Vec::new();
    for acct in accounts.iter().filter(|a| a.active) {
        let label = acct.line.label();
        let password = match decrypt_password(&s, acct) {
            Ok(p) => p,
            Err((_, e)) => {
                errors.push(format!("{label}: {e}"));
                continue;
            }
        };
        match royaltracker_core::discover_account(
            s.repo.as_ref(),
            s.rcg_basic_auth_b64.as_ref(),
            &user.db_user,
            acct,
            &password,
            None,
        )
        .await
        {
            Ok(d) => {
                total += d.report.persisted;
                errors.extend(d.report.errors.into_iter().map(|e| format!("{label}: {e}")));
            }
            Err(e) => errors.push(format!("{label}: {e}")),
        }
    }
    Ok(Json(RefreshResponse {
        bookings_discovered: total,
        errors,
    }))
}

pub async fn list_accounts(
    State(s): State<Arc<AppState>>,
    user: AuthedUser,
) -> Result<Json<Vec<AccountDto>>, ApiError> {
    let accts = s
        .repo
        .list_cruise_accounts(user.db_user.id)
        .await
        .map_err(db_err)?;
    Ok(Json(accts.iter().map(AccountDto::from).collect()))
}

pub async fn delete_account(
    State(s): State<Arc<AppState>>,
    user: AuthedUser,
    Path(line): Path<String>,
) -> Result<StatusCode, ApiError> {
    let line = line
        .parse::<CruiseLine>()
        .map_err(|_| (StatusCode::NOT_FOUND, "account not found".into()))?;
    let removed = s
        .repo
        .remove_cruise_account(user.db_user.id, line)
        .await
        .map_err(db_err)?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::NOT_FOUND, "account not found".into()))
    }
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
    if !s
        .repo
        .user_owns_reservation(user.db_user.id, &body.reservation_id)
        .await
        .map_err(db_err)?
    {
        return Err((StatusCode::NOT_FOUND, "booking not found".into()));
    }
    let all = s.repo.list_bookings().await.map_err(db_err)?;
    let booking = all
        .into_iter()
        .find(|b| b.reservation_id == body.reservation_id)
        .ok_or((StatusCode::NOT_FOUND, "booking not found".into()))?;
    if booking.has_departed(Utc::now().date_naive()) {
        return Err((StatusCode::CONFLICT, "this cruise has already sailed".into()));
    }

    if booking.brand == Brand::Princess {
        let persisted = royaltracker_core::refresh_princess_catalog(
            s.princess.as_ref(),
            s.princess_extras.as_ref(),
            s.repo.as_ref(),
            &booking,
        )
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("catalog: {e}")))?;
        return Ok(Json(CatalogRefreshResponse { products_persisted: persisted }));
    }
    // GraphQL endpoint is anonymous (appkey only), so we don't need the user's
    // credentials at all. Build a client with the booking's brand.
    let api_cfg = CruiseClientConfig::web(
        booking.brand,
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
    pub accounts: Vec<AccountDto>,
}

pub async fn get_me(
    State(s): State<Arc<AppState>>,
    user: AuthedUser,
) -> Result<Json<MeResponse>, ApiError> {
    let accts = s
        .repo
        .list_cruise_accounts(user.db_user.id)
        .await
        .map_err(db_err)?;
    Ok(Json(MeResponse {
        tg_id: user.tg.id,
        tg_username: user.tg.username,
        tg_first_name: user.tg.first_name,
        accounts: accts.iter().map(AccountDto::from).collect(),
    }))
}

#[derive(Serialize)]
pub struct BookingDto {
    pub reservation_id: String,
    pub brand: String,
    pub brand_label: String,
    pub ship_name: Option<String>,
    pub voyage_code: Option<String>,
    /// ISO currency of the booking's prices; `None` = USD.
    pub currency: Option<String>,
    pub ship_code: String,
    pub sail_date: chrono::NaiveDate,
    pub nights: Option<i32>,
    pub package_code: Option<String>,
    /// Stateroom as RCG displays it: a real cabin, or "GTY" while unassigned.
    pub stateroom: Option<String>,
    /// The real cabin a "GTY" booking is currently assigned to, recovered from
    /// purchased-order records. Present only for GTY bookings we could resolve.
    pub assigned_stateroom: Option<String>,
    /// Deck derived from the effective (assigned or displayed) cabin, when known.
    pub deck: Option<i32>,
    /// Coarse height-tier label (e.g. "mid-ship height") when the ship's deck
    /// range is known; null otherwise.
    pub height_tier: Option<String>,
    /// Hedged human location notes (motion, top/bottom-deck hints).
    #[serde(default)]
    pub location_notes: Vec<String>,
    /// Other royaltracker users subscribed to the same reservation (i.e.
    /// partners/family on the same cruise). Excludes the caller. Empty when
    /// the booking is solo. Each entry's `display` is the safest non-PII
    /// label we have: `@telegram_username` if set, else `null` (and the UI
    /// just renders a head-count).
    #[serde(default)]
    pub shared_with: Vec<SharedSubscriberDto>,
    /// The ship has sailed but the cruise isn't over: still listed, no longer
    /// price-checked.
    pub sailing_now: bool,
}

#[derive(Serialize)]
pub struct SharedSubscriberDto {
    pub display: Option<String>,
}

/// The caller's bookings that haven't ended yet. Ended cruises are hidden
/// everywhere in the Mini App.
async fn current_bookings_for(
    s: &AppState,
    user_id: i64,
    today: chrono::NaiveDate,
) -> Result<Vec<royaltracker_types::Booking>, ApiError> {
    let mut mine = s.repo.list_bookings_for_user(user_id).await.map_err(db_err)?;
    mine.retain(|b| !b.has_ended(today));
    Ok(mine)
}

pub async fn list_bookings(
    State(s): State<Arc<AppState>>,
    user: AuthedUser,
) -> Result<Json<Vec<BookingDto>>, ApiError> {
    let today = Utc::now().date_naive();
    let mine = current_bookings_for(&s, user.db_user.id, today).await?;
    let mut out = Vec::with_capacity(mine.len());
    for b in mine {
        let sailing_now = b.has_departed(today);
        let subs = s
            .repo
            .list_subscribers_for_reservation(&b.reservation_id)
            .await
            .map_err(db_err)?;
        let shared_with: Vec<SharedSubscriberDto> = subs
            .into_iter()
            .filter(|s| s.user_id != user.db_user.id)
            .map(|s| SharedSubscriberDto {
                display: s.telegram_username.map(|u| format!("@{u}")),
            })
            .collect();
        // Derive location facts from the effective cabin (recovered GTY room, or
        // the displayed room when it's a real number rather than "GTY").
        let effective_cabin = b
            .assigned_stateroom
            .as_deref()
            .or_else(|| b.stateroom.as_deref().filter(|r| *r != "GTY"));
        let loc = effective_cabin.and_then(|c| royaltracker_types::cabin_location(b.brand, &b.ship_code, c));
        let (deck, height_tier, location_notes) = match loc {
            Some(l) => (
                Some(l.deck as i32),
                l.tier.map(|t| t.label().to_string()),
                l.notes,
            ),
            None => (None, None, Vec::new()),
        };
        out.push(BookingDto {
            reservation_id: b.reservation_id,
            brand: b.brand.to_string(),
            brand_label: b.brand.label().to_string(),
            ship_name: royaltracker_types::ship_name(b.brand, &b.ship_code).map(str::to_string),
            voyage_code: b.voyage_code,
            currency: b.currency,
            ship_code: b.ship_code,
            sail_date: b.sail_date,
            nights: b.nights,
            package_code: b.package_code,
            stateroom: b.stateroom,
            assigned_stateroom: b.assigned_stateroom,
            deck,
            height_tier,
            location_notes,
            shared_with,
            sailing_now,
        });
    }
    Ok(Json(out))
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
    user: AuthedUser,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<CatalogDto>>, ApiError> {
    let entries = s
        .repo
        .search_catalog(user.db_user.id, &q.q, q.limit)
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
    user: AuthedUser,
    Query(q): Query<BrowseQuery>,
) -> Result<Json<Vec<CatalogDto>>, ApiError> {
    if !s
        .repo
        .user_owns_reservation(user.db_user.id, &q.reservation_id)
        .await
        .map_err(db_err)?
    {
        return Err((StatusCode::NOT_FOUND, "booking not found".into()));
    }
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
    pub alert_mode: String,
    pub alert_threshold: Option<f64>,
    pub latest_price: Option<f64>,
    /// When the `latest_price` snapshot was taken. The UI flags this as stale
    /// when it's older than the daily scrape cadence, so a silently-failed
    /// fetch (which writes no snapshot) doesn't masquerade as a live price.
    pub latest_fetched_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn list_watched(
    State(s): State<Arc<AppState>>,
    user: AuthedUser,
) -> Result<Json<Vec<WatchedDto>>, ApiError> {
    let mine = current_bookings_for(&s, user.db_user.id, Utc::now().date_naive()).await?;
    let my_reservations: std::collections::HashSet<String> =
        mine.into_iter().map(|b| b.reservation_id).collect();

    let ws = s.repo.list_active_watched().await.map_err(db_err)?;
    let mut out = Vec::with_capacity(ws.len());
    for w in ws {
        if !my_reservations.contains(&w.reservation_id) {
            continue;
        }
        let latest = s.repo.latest_snapshot(w.id).await.map_err(db_err)?;
        let latest_price = latest.as_ref().and_then(|s| s.adult_promo_price);
        let latest_fetched_at = latest.map(|s| s.fetched_at);
        out.push(WatchedDto {
            id: w.id,
            reservation_id: w.reservation_id,
            category_prefix: w.category_prefix,
            product_code: w.product_code,
            label: w.label,
            alert_mode: w.alert_mode.to_string(),
            alert_threshold: w.alert_threshold,
            latest_price,
            latest_fetched_at,
        });
    }
    Ok(Json(out))
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
    user: AuthedUser,
    Json(body): Json<AddWatchedBody>,
) -> Result<Json<i64>, ApiError> {
    if !s
        .repo
        .user_owns_reservation(user.db_user.id, &body.reservation_id)
        .await
        .map_err(db_err)?
    {
        return Err((StatusCode::NOT_FOUND, "booking not found".into()));
    }
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

    // Seed a baseline price from the catalog cache so the user sees a price
    // immediately (and the chart isn't empty until tomorrow's scrape). Only
    // seed if we don't already have one — otherwise re-adding a removed watch
    // would dump a stale catalog price into the history.
    let already_has_snapshot = s.repo.latest_snapshot(id).await.map_err(db_err)?.is_some();
    if !already_has_snapshot {
        if let Ok(entries) = s.repo.list_catalog_by_reservation(&body.reservation_id).await {
            if let Some(entry) = entries
                .into_iter()
                .find(|e| e.product_code == body.product_code)
            {
                if let Some(price) = entry.starting_price {
                    let snap = royaltracker_types::PriceSnapshot {
                        watched_id: id,
                        fetched_at: Utc::now(),
                        adult_promo_price: Some(price),
                        child_promo_price: None,
                        raw_response: serde_json::json!({ "source": "catalog_baseline" }),
                    };
                    if let Err(e) = s.repo.insert_snapshot(&snap).await {
                        tracing::warn!(error = %e, watched_id = id, "seed baseline failed");
                    }
                }
            }
        }
    }

    Ok(Json(id))
}

pub async fn remove_watched(
    State(s): State<Arc<AppState>>,
    user: AuthedUser,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if !watch_belongs_to_user(&*s.repo, user.db_user.id, id).await? {
        return Err((StatusCode::NOT_FOUND, "watch not found".into()));
    }
    s.repo.deactivate_watched(id).await.map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct AlertConfigBody {
    pub alert_mode: String,
    pub alert_threshold: Option<f64>,
}

pub async fn set_alert(
    State(s): State<Arc<AppState>>,
    user: AuthedUser,
    Path(id): Path<i64>,
    Json(body): Json<AlertConfigBody>,
) -> Result<StatusCode, ApiError> {
    if !watch_belongs_to_user(&*s.repo, user.db_user.id, id).await? {
        return Err((StatusCode::NOT_FOUND, "watch not found".into()));
    }
    let mode = body
        .alert_mode
        .parse::<AlertMode>()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("alert_mode: {e}")))?;
    let threshold = match mode {
        AlertMode::BelowThreshold => Some(body.alert_threshold.ok_or((
            StatusCode::BAD_REQUEST,
            "alert_threshold required for below_threshold".into(),
        ))?),
        AlertMode::AnyDrop => None,
    };
    s.repo
        .set_watch_alert(id, mode, threshold)
        .await
        .map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn watch_belongs_to_user(
    repo: &dyn PriceRepo,
    user_id: i64,
    watch_id: i64,
) -> Result<bool, ApiError> {
    let watches = repo.list_active_watched().await.map_err(db_err)?;
    let Some(w) = watches.into_iter().find(|w| w.id == watch_id) else {
        return Ok(false);
    };
    repo.user_owns_reservation(user_id, &w.reservation_id)
        .await
        .map_err(db_err)
}

pub async fn history(
    State(s): State<Arc<AppState>>,
    user: AuthedUser,
    Path(watched_id): Path<i64>,
) -> Result<Json<Vec<HistoryPoint>>, ApiError> {
    if !watch_belongs_to_user(&*s.repo, user.db_user.id, watched_id).await? {
        return Err((StatusCode::NOT_FOUND, "not found".into()));
    }
    let points = s
        .repo
        .snapshot_history(watched_id, 365)
        .await
        .map_err(db_err)?;
    Ok(Json(points))
}

#[derive(Deserialize)]
pub struct DeckPlanQuery {
    /// Brand key: "royal", "celebrity" or "princess".
    pub brand: String,
    /// Two-letter ship code (e.g. "WN").
    pub ship: String,
    pub deck: i32,
}

#[derive(Serialize)]
pub struct DeckPlanResponse {
    /// Absolute cruisedeckplans image URL (hotlinked by the client, not rehosted).
    pub image_url: String,
    /// The deck page on cruisedeckplans, for attribution / "open full".
    pub source_url: String,
}

/// Resolve the deck-plan image for a ship + deck, cached after first lookup.
/// On a cache miss we do one anonymous scrape of cruisedeckplans to read the
/// versioned image filename, then persist it.
pub async fn deck_plan(
    State(s): State<Arc<AppState>>,
    user: AuthedUser,
    Query(q): Query<DeckPlanQuery>,
) -> Result<Json<DeckPlanResponse>, ApiError> {
    if !(1..=25).contains(&q.deck) {
        return Err((StatusCode::BAD_REQUEST, "invalid deck".into()));
    }
    let brand = q
        .brand
        .parse::<Brand>()
        .map_err(|_| (StatusCode::BAD_REQUEST, "unknown brand".into()))?;
    if brand == Brand::Princess {
        return Err((StatusCode::NOT_FOUND, "deck plans not available".into()));
    }
    // Ownership gate: the caller must have a booking on this ship. Otherwise the
    // endpoint is an open proxy for arbitrary cruisedeckplans scrapes.
    let owns_ship = s
        .repo
        .list_bookings_for_user(user.db_user.id)
        .await
        .map_err(db_err)?
        .iter()
        .any(|b| b.brand == brand && b.ship_code.eq_ignore_ascii_case(&q.ship));
    if !owns_ship {
        return Err((StatusCode::NOT_FOUND, "not found".into()));
    }
    let Some(name) = royaltracker_types::ship_name(brand, &q.ship) else {
        return Err((StatusCode::NOT_FOUND, "unknown ship".into()));
    };
    let slug = name.replace(' ', "-");
    let source_url = format!(
        "https://www.cruisedeckplans.com/ships/deckbydeck.php?ship={slug}&deck={}",
        q.deck
    );

    // Deck plans never change, so a cached URL is authoritative.
    if let Some(dp) = s.repo.get_deck_plan(brand, &q.ship, q.deck).await.map_err(db_err)? {
        return Ok(Json(DeckPlanResponse {
            image_url: dp.image_url,
            source_url,
        }));
    }

    // Cache miss: anonymous scrape (appkey-only client, no user creds needed).
    let client = CruiseClient::new(CruiseClientConfig::web(
        brand,
        String::new(),
        String::new(),
        s.rcg_basic_auth_b64.as_ref().clone(),
    ))
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("client: {e}")))?;

    let image_url = client
        .fetch_deck_image_url(&slug, q.deck as u16)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("deckplan: {e}")))?
        .ok_or((StatusCode::NOT_FOUND, "deck plan not found".to_string()))?;

    let dp = DeckPlan {
        brand,
        ship_code: q.ship.clone(),
        deck: q.deck,
        image_url: image_url.clone(),
        sourced_at: Utc::now(),
    };
    if let Err(e) = s.repo.upsert_deck_plan(&dp).await {
        tracing::warn!(error = %e, "deck_plan cache upsert failed");
    }

    Ok(Json(DeckPlanResponse {
        image_url,
        source_url,
    }))
}

/// User-facing message for a completed register/discovery run.
fn register_message(line: CruiseLine, persisted: usize, logins_ok: usize, errors: &[String]) -> String {
    let plural = if persisted == 1 { "" } else { "s" };
    if line == CruiseLine::Princess {
        if errors.iter().any(|e| e == royaltracker_core::PRINCESS_NO_BOOKING_MSG) {
            return royaltracker_core::PRINCESS_NO_BOOKING_MSG.to_string();
        }
        if logins_ok > 0 && errors.is_empty() {
            return format!("Linked. Discovered {persisted} booking{plural}.");
        }
    } else if logins_ok > 0 && errors.is_empty() {
        return format!(
            "Linked. Discovered {persisted} booking{plural} across {logins_ok} brand{}.",
            if logins_ok == 1 { "" } else { "s" },
        );
    }
    if logins_ok > 0 {
        format!(
            "Linked partially. {persisted} bookings persisted; some brands errored: {}",
            errors.join("; "),
        )
    } else {
        format!(
            "Credentials saved, but no login succeeded: {}. \
             Re-register to overwrite, or wait if rate-limited.",
            errors.join("; "),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use royaltracker_types::{CruiseAccount, CruiseLine};

    fn sample_account() -> CruiseAccount {
        CruiseAccount {
            id: 1,
            user_id: 2,
            line: CruiseLine::Princess,
            login: "LOGINID".into(),
            password_ct: vec![1, 2, 3],
            password_nonce: vec![4, 5, 6],
            active: true,
            last_login_at: None,
            last_login_error: None,
        }
    }

    #[test]
    fn register_message_per_line() {
        let m = |l, p, ok, e: &[&str]| register_message(l, p, ok, &e.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(m(CruiseLine::Rcg, 2, 2, &[]), "Linked. Discovered 2 bookings across 2 brands.");
        assert_eq!(m(CruiseLine::Princess, 1, 1, &[]), "Linked. Discovered 1 booking.");
        assert_eq!(
            m(CruiseLine::Princess, 0, 1, &[royaltracker_core::PRINCESS_NO_BOOKING_MSG]),
            royaltracker_core::PRINCESS_NO_BOOKING_MSG
        );
        assert!(m(CruiseLine::Princess, 0, 0, &["bad"]).starts_with("Credentials saved"));
    }

    #[test]
    fn register_validation() {
        assert!(validate_register(CruiseLine::Rcg, "a@b.c", "pw").is_ok());
        assert!(validate_register(CruiseLine::Rcg, "nope", "pw").is_err());
        assert!(validate_register(CruiseLine::Princess, "LOGINID", "pw").is_ok());
        assert!(validate_register(CruiseLine::Princess, "x", "").is_err());
    }

    #[test]
    fn account_dto_never_contains_secrets() {
        let dto = AccountDto::from(&sample_account());
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("password"));
        assert!(!json.contains("_ct"));
        assert_eq!(dto.label, "Princess");
        assert_eq!(dto.cruise_line, "princess");
    }

    #[test]
    fn register_body_defaults_and_alias() {
        let b: RegisterBody = serde_json::from_str(r#"{"email":"a@b.c","password":"p"}"#).unwrap();
        assert_eq!(b.cruise_line, CruiseLine::Rcg);
        assert_eq!(b.login, "a@b.c");
        let b: RegisterBody =
            serde_json::from_str(r#"{"cruise_line":"princess","login":"X","password":"p"}"#).unwrap();
        assert_eq!(b.cruise_line, CruiseLine::Princess);
    }
}
