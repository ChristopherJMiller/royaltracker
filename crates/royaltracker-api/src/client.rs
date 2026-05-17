use crate::graphql::{fetch_categories, fetch_products_in_category, Category, GraphqlProduct};
use royaltracker_types::Brand;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, instrument};

use crate::auth::{build_token_state, OAuthTokenResponse, TokenState};
use crate::catalog::ProductPrice;
use crate::error::ApiError;
use crate::WEB_APP_KEY;

#[derive(Debug, Clone)]
pub struct CruiseClientConfig {
    pub brand: Brand,
    pub username: String,
    pub password: String,
    /// Hardcoded `Authorization: Basic <client_id:secret>` payload from the JS bundle.
    /// Pulled from jdeath/CheckRoyalCaribbeanPrice during Phase 0. Configurable so it
    /// can be rotated without a rebuild when RCG updates the bundle.
    pub basic_auth_b64: String,
    pub app_key: String,
    pub user_agent: String,
}

/// UA must match the TLS fingerprint set by `Emulation::Chrome145` below — Akamai
/// Bot Manager fingerprints both and flags mismatches (this is what broke us on
/// 2026-05-17: Firefox UA on top of Chrome JA3 started getting 403s/404s).
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";

impl CruiseClientConfig {
    pub fn web(brand: Brand, username: String, password: String, basic_auth_b64: String) -> Self {
        Self {
            brand,
            username,
            password,
            basic_auth_b64,
            app_key: WEB_APP_KEY.to_string(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
        }
    }
}

pub struct CruiseClient {
    cfg: CruiseClientConfig,
    http: wreq::Client,
    token: Arc<Mutex<Option<TokenState>>>,
    warmed: Arc<Mutex<bool>>,
}

impl CruiseClient {
    pub fn new(cfg: CruiseClientConfig) -> Result<Self, ApiError> {
        let http = wreq::Client::builder()
            .emulation(wreq_util::Emulation::Chrome145)
            .timeout(std::time::Duration::from_secs(30))
            .cookie_store(true)
            .build()?;
        Ok(Self {
            cfg,
            http,
            token: Arc::new(Mutex::new(None)),
            warmed: Arc::new(Mutex::new(false)),
        })
    }

    pub fn brand(&self) -> Brand {
        self.cfg.brand
    }

    /// Hit the brand homepage once per client to seed the cookie jar with Akamai's
    /// `_abck` and `bm_sz` cookies. Without these, the OAuth POST gets a 403 from
    /// Bot Manager (observed 2026-05-17). The first request returns an untrusted
    /// `_abck=...~-1~...` but the origin still issues a JWT — Phase 0 confirmed
    /// the auth flow is "detect mode, not enforce."
    async fn warm_up(&self) -> Result<(), ApiError> {
        {
            let g = self.warmed.lock().await;
            if *g {
                return Ok(());
            }
        }
        let url = format!("https://{}/", self.cfg.brand.host());
        let resp = self
            .http
            .get(&url)
            .header("User-Agent", &self.cfg.user_agent)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await?;
        debug!(status = %resp.status(), "warm-up GET complete");
        // Drain the body so the connection can be reused.
        let _ = resp.text().await;
        let mut g = self.warmed.lock().await;
        *g = true;
        Ok(())
    }

    #[instrument(skip(self), fields(brand = %self.cfg.brand))]
    pub async fn login(&self) -> Result<TokenState, ApiError> {
        self.warm_up().await?;

        let host = self.cfg.brand.host();
        let url = format!("https://{}/auth/oauth2/access_token", host);

        let form = [
            ("grant_type", "password"),
            ("username", self.cfg.username.as_str()),
            ("password", self.cfg.password.as_str()),
            ("scope", "openid profile email vdsid"),
        ];

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Basic {}", self.cfg.basic_auth_b64))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", &self.cfg.user_agent)
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Origin", format!("https://{}", host))
            .header("Referer", format!("https://{}/account/signin", host))
            .form(&form)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::Status {
                status: status.as_u16(),
                body,
            });
        }

        let token: OAuthTokenResponse = resp.json().await?;
        let state = build_token_state(token)?;
        debug!(account_id = %state.account_id, "login succeeded");

        let mut guard = self.token.lock().await;
        *guard = Some(state.clone());
        Ok(state)
    }

    async fn ensure_token(&self) -> Result<TokenState, ApiError> {
        let needs_refresh = {
            let g = self.token.lock().await;
            match g.as_ref() {
                None => true,
                Some(t) => t.is_expired(chrono::Duration::seconds(60)),
            }
        };
        if needs_refresh {
            self.login().await
        } else {
            let g = self.token.lock().await;
            Ok(g.as_ref().expect("just checked").clone())
        }
    }

    fn auth_headers(&self, token: &TokenState) -> Vec<(&'static str, String)> {
        vec![
            ("Access-Token", token.access_token.clone()),
            ("AppKey", self.cfg.app_key.clone()),
            ("Account-Id", token.account_id.clone()),
            ("User-Agent", self.cfg.user_agent.clone()),
            ("Accept", "application/json".to_string()),
        ]
    }

    #[instrument(skip(self))]
    pub async fn list_bookings(&self) -> Result<Vec<BookingSummary>, ApiError> {
        self.list_bookings_for(self.cfg.brand).await
    }

    /// Fetch bookings filtered to a specific brand. Phase 0 verified the same JWT
    /// (issued via either auth host) works for both `?brand=R` and `?brand=C`.
    pub async fn list_bookings_for(&self, brand: Brand) -> Result<Vec<BookingSummary>, ApiError> {
        let token = self.ensure_token().await?;
        let url = format!(
            "https://{}/v1/profileBookings/enriched/{}?brand={}",
            brand.api_host(),
            token.account_id,
            brand.code()
        );

        let mut req = self.http.get(&url);
        for (k, v) in self.auth_headers(&token) {
            req = req.header(k, v);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            tracing::warn!(brand = ?brand, status = %status, body = %body, "list_bookings_for: non-success");
            return Err(ApiError::Status {
                status: status.as_u16(),
                body,
            });
        }

        let body_text = resp.text().await?;
        tracing::debug!(brand = ?brand, body_len = body_text.len(), body = %body_text, "list_bookings_for: raw body");

        let value: serde_json::Value = serde_json::from_str(&body_text)?;
        let bookings = match value
            .get("payload")
            .and_then(|p| p.get("profileBookings"))
        {
            Some(b) => match serde_json::from_value::<Vec<BookingSummary>>(b.clone()) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(brand = ?brand, error = %e, "profileBookings deserialize failed");
                    Vec::new()
                }
            },
            None => {
                tracing::warn!(brand = ?brand, "profileBookings missing in response");
                Vec::new()
            }
        };
        tracing::info!(brand = ?brand, count = bookings.len(), "list_bookings_for: ok");
        Ok(bookings)
    }

    /// Fetch bookings across both brands. Returns a Vec of `(brand, summary)` pairs so
    /// callers can persist with the correct brand without losing it.
    /// Fetch the full product catalog (categories + paginated products) for a sailing.
    /// Uses the anonymous GraphQL endpoint — no JWT required, only `appkey`.
    /// Returns Vec<(category_id, category_name, product)>.
    pub async fn fetch_catalog(
        &self,
        ship_code: &str,
        sail_date: chrono::NaiveDate,
        passenger_id: Option<&str>,
        reservation_id: Option<&str>,
    ) -> Result<Vec<(Category, GraphqlProduct)>, ApiError> {
        let categories = fetch_categories(
            &self.http,
            &self.cfg.app_key,
            &self.cfg.user_agent,
            ship_code,
            sail_date,
        )
        .await?;
        tracing::info!(count = categories.len(), "fetched categories");
        let mut out = Vec::new();
        for cat in categories {
            let products = fetch_products_in_category(
                &self.http,
                &self.cfg.app_key,
                &self.cfg.user_agent,
                ship_code,
                sail_date,
                &cat.id,
                passenger_id,
                reservation_id,
                "USD",
            )
            .await?;
            tracing::info!(category = %cat.id, count = products.len(), "fetched products");
            for p in products {
                out.push((cat.clone(), p));
            }
        }
        Ok(out)
    }

    pub async fn list_all_bookings(&self) -> Result<Vec<(Brand, BookingSummary)>, ApiError> {
        let mut out = Vec::new();
        for brand in [Brand::Royal, Brand::Celebrity] {
            match self.list_bookings_for(brand).await {
                Ok(bs) => out.extend(bs.into_iter().map(|b| (brand, b))),
                Err(e) => tracing::warn!(brand = ?brand, error = %e, "list_bookings_for failed"),
            }
        }
        Ok(out)
    }

    /// Fetch a single product's personalized price from catalog/v2.
    /// Mirrors jdeath's headline call.
    #[instrument(skip(self))]
    pub async fn fetch_product_price(
        &self,
        ship_code: &str,
        category_prefix: &str,
        product_code: &str,
        reservation_id: &str,
        passenger_id: &str,
        start_date: chrono::NaiveDate,
    ) -> Result<ProductPrice, ApiError> {
        let token = self.ensure_token().await?;
        let url = format!(
            "https://{api}/en/{seg}/web/commerce-api/catalog/v2/{ship}/categories/{cat}/products/{prod}?reservationId={res}&passengerId={pax}&startDate={start}&currencyIso=USD",
            api = self.cfg.brand.api_host(),
            seg = self.cfg.brand.url_segment(),
            ship = ship_code,
            cat = category_prefix,
            prod = product_code,
            res = reservation_id,
            pax = passenger_id,
            start = start_date.format("%Y-%m-%d"),
        );

        let mut req = self.http.get(&url);
        for (k, v) in self.auth_headers(&token) {
            req = req.header(k, v);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::Status {
                status: status.as_u16(),
                body,
            });
        }

        let raw: serde_json::Value = resp.json().await?;
        Ok(ProductPrice::from_raw(raw))
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BookingSummary {
    /// Live API uses `bookingId`. Older docs / brief used `reservationId` — accept that too
    /// in case future schema changes.
    #[serde(default, alias = "bookingId")]
    pub reservation_id: Option<String>,
    #[serde(default)]
    pub ship_code: Option<String>,
    /// Sail date in the wire format `YYYYMMDD` (no dashes) per Phase 0 ground truth.
    #[serde(default)]
    pub sail_date: Option<String>,
    /// The JSON has BOTH `passengerId` and `masterPassengerId` with the same value.
    /// Map only `passengerId` to avoid serde's duplicate-field error when both keys
    /// land in the same payload.
    #[serde(default, alias = "passengerId")]
    pub primary_passenger_id: Option<String>,
    #[serde(default)]
    pub brand: Option<String>,
    #[serde(default)]
    pub royaltracker_name: Option<String>,
    #[serde(default)]
    pub number_of_nights: Option<i32>,
    #[serde(default)]
    pub package_code: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
