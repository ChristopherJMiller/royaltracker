//! Public (no-login) price client. Appkey-only, reservationId OMITTED — the path
//! that returns real promotional prices keyed by ship + sail date. Reuses the
//! Chrome-emulated wreq stack + Akamai warm-up. Paced and backed-off so a broad
//! public sweep doesn't anger the origin.

use chrono::NaiveDate;
use royaltracker_types::Brand;
use std::collections::HashSet;
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::client::{build_emulated_client, warm_up_host};
use crate::error::ApiError;
use crate::graphql::{fetch_categories, fetch_products_in_category, parse_money_cents};
use crate::WEB_APP_KEY;

#[derive(Debug, Clone)]
pub struct PacingConfig {
    pub min_interval: Duration,
    pub jitter: (Duration, Duration),
    pub max_retries: u32,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
    pub cooldown_after_challenge: Duration,
}

impl PacingConfig {
    /// Build from raw millisecond values (as they arrive from config).
    #[allow(clippy::too_many_arguments)]
    pub fn from_millis(
        min_interval_ms: u64,
        jitter_lo_ms: u64,
        jitter_hi_ms: u64,
        max_retries: u32,
        base_backoff_ms: u64,
        max_backoff_ms: u64,
        cooldown_after_challenge_ms: u64,
    ) -> Self {
        Self {
            min_interval: Duration::from_millis(min_interval_ms),
            jitter: (
                Duration::from_millis(jitter_lo_ms),
                Duration::from_millis(jitter_hi_ms),
            ),
            max_retries,
            base_backoff: Duration::from_millis(base_backoff_ms),
            max_backoff: Duration::from_millis(max_backoff_ms),
            cooldown_after_challenge: Duration::from_millis(cooldown_after_challenge_ms),
        }
    }
}

impl Default for PacingConfig {
    fn default() -> Self {
        Self {
            min_interval: Duration::from_millis(3000),
            jitter: (Duration::from_millis(2000), Duration::from_millis(8000)),
            max_retries: 4,
            base_backoff: Duration::from_secs(30),
            max_backoff: Duration::from_secs(900),
            cooldown_after_challenge: Duration::from_secs(600),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PublicClientConfig {
    pub app_key: String,
    pub user_agent: String,
    pub pacing: PacingConfig,
}

impl PublicClientConfig {
    pub fn new(user_agent: String, pacing: PacingConfig) -> Self {
        Self {
            app_key: WEB_APP_KEY.to_string(),
            user_agent,
            pacing,
        }
    }
}

/// One public catalog product for a sailing. Prices are exact integer cents to
/// avoid float drift before the storage boundary.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PublicProduct {
    pub category_id: String,
    pub category_name: String,
    pub product_code: String,
    pub title: Option<String>,
    /// The fluctuating sale price; None when no active promo (sale ended).
    pub promo_cents: Option<i64>,
    pub base_cents: Option<i64>,
    pub promo_label: Option<String>,
    pub base_label: Option<String>,
    pub currency: Option<String>,
    pub unit_label: Option<String>,
}

impl PublicProduct {
    pub fn promo_dollars(&self) -> Option<f64> {
        self.promo_cents.map(|c| c as f64 / 100.0)
    }
    pub fn base_dollars(&self) -> Option<f64> {
        self.base_cents.map(|c| c as f64 / 100.0)
    }
}

/// Result of a public fetch. `PlannerNotOpen` is the legitimate "0 products"
/// state for a too-far-out sailing — NOT a bot challenge (challenges surface as
/// `Err`).
#[derive(Debug, Clone)]
pub enum PublicFetch {
    Products(Vec<PublicProduct>),
    PlannerNotOpen,
}

struct RateLimiter {
    last: Mutex<Option<Instant>>,
    pacing: PacingConfig,
}

impl RateLimiter {
    /// Sleep so consecutive requests are at least `min_interval` apart plus a
    /// random jitter, making the traffic look less metronomic to Akamai.
    async fn wait(&self) {
        let extra = jitter_between(self.pacing.jitter.0, self.pacing.jitter.1);
        let mut guard = self.last.lock().await;
        if let Some(prev) = *guard {
            let target = self.pacing.min_interval + extra;
            let elapsed = prev.elapsed();
            if elapsed < target {
                tokio::time::sleep(target - elapsed).await;
            }
        } else {
            tokio::time::sleep(extra).await;
        }
        *guard = Some(Instant::now());
    }
}

/// Cheap, non-crypto jitter from wall-clock nanos — good enough for spacing.
fn jitter_between(lo: Duration, hi: Duration) -> Duration {
    if hi <= lo {
        return lo;
    }
    let span = (hi - lo).as_millis().max(1) as u64;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    lo + Duration::from_millis(nanos % span)
}

/// Retryable = origin pushback (429/403/5xx) or an Akamai bot challenge, where
/// backing off and trying again is the right move.
fn is_retryable(e: &ApiError) -> bool {
    match e {
        ApiError::Http(_) => true,
        ApiError::Status { status, body } => {
            matches!(status, 429 | 403 | 500 | 502 | 503 | 504) || is_bot_challenge(body)
        }
        _ => false,
    }
}

/// Heuristic detection of an Akamai/CDN interstitial (vs a genuine empty catalog).
pub fn is_bot_challenge(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    b.contains("access denied")
        || b.contains("reference #")
        || b.contains("_abck")
        || b.contains("bot manager")
        || b.contains("captcha")
}

pub struct PublicClient {
    cfg: PublicClientConfig,
    http: wreq::Client,
    limiter: RateLimiter,
    warmed: Mutex<HashSet<Brand>>,
}

impl PublicClient {
    pub fn new(cfg: PublicClientConfig) -> Result<Self, ApiError> {
        let http = build_emulated_client()?;
        let pacing = cfg.pacing.clone();
        Ok(Self {
            cfg,
            http,
            limiter: RateLimiter {
                last: Mutex::new(None),
                pacing,
            },
            warmed: Mutex::new(HashSet::new()),
        })
    }

    async fn ensure_warm(&self, brand: Brand) -> Result<(), ApiError> {
        {
            let g = self.warmed.lock().await;
            if g.contains(&brand) {
                return Ok(());
            }
        }
        warm_up_host(&self.http, &self.cfg.user_agent, brand.host()).await?;
        self.warmed.lock().await.insert(brand);
        Ok(())
    }

    async fn retry<T, F, Fut>(&self, mut f: F) -> Result<T, ApiError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, ApiError>>,
    {
        let mut attempt = 0u32;
        loop {
            match f().await {
                Ok(v) => return Ok(v),
                Err(e) if is_retryable(&e) && attempt < self.cfg.pacing.max_retries => {
                    let challenged = matches!(&e, ApiError::Status { body, .. } if is_bot_challenge(body));
                    let mut backoff = self.cfg.base_backoff_pow(attempt);
                    if challenged {
                        backoff = backoff.max(self.cfg.pacing.cooldown_after_challenge);
                    }
                    tracing::warn!(attempt, challenged, backoff_s = backoff.as_secs(), error = %e, "public fetch retry");
                    tokio::time::sleep(backoff).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// List the bookable sail dates for a ship (public voyages endpoint). Powers
    /// the date picker before anything has been tracked.
    pub async fn fetch_public_sailings(
        &self,
        brand: Brand,
        ship_code: &str,
    ) -> Result<Vec<NaiveDate>, ApiError> {
        self.ensure_warm(brand).await?;
        self.limiter.wait().await;
        let url = format!(
            "https://aws-prd.api.rccl.com/en/{}/web/v3/ships/{}/voyages",
            brand.url_segment(),
            ship_code
        );
        let text = self
            .retry(|| async {
                let resp = self
                    .http
                    .get(&url)
                    .header("User-Agent", &self.cfg.user_agent)
                    .header("Accept", "application/json")
                    .header("appkey", &self.cfg.app_key)
                    .send()
                    .await?;
                let status = resp.status();
                let body = resp.text().await?;
                if !status.is_success() {
                    return Err(ApiError::Status {
                        status: status.as_u16(),
                        body,
                    });
                }
                Ok(body)
            })
            .await?;
        let v: serde_json::Value = serde_json::from_str(&text)?;
        let mut dates: Vec<NaiveDate> = v
            .get("payload")
            .and_then(|p| p.get("voyages"))
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.get("sailDate").and_then(|d| d.as_str()))
                    .filter_map(|s| {
                        NaiveDate::parse_from_str(s, "%Y%m%d")
                            .or_else(|_| NaiveDate::parse_from_str(s, "%Y-%m-%d"))
                            .ok()
                    })
                    .collect()
            })
            .unwrap_or_default();
        dates.sort();
        dates.dedup();
        Ok(dates)
    }

    /// Fetch every public catalog product for a sailing (appkey-only, no
    /// reservation). Paced between category calls, retried with backoff.
    pub async fn fetch_public_products(
        &self,
        brand: Brand,
        ship_code: &str,
        sail_date: NaiveDate,
    ) -> Result<PublicFetch, ApiError> {
        self.ensure_warm(brand).await?;
        self.limiter.wait().await;
        let cats = self
            .retry(|| {
                fetch_categories(
                    &self.http,
                    &self.cfg.app_key,
                    &self.cfg.user_agent,
                    brand,
                    ship_code,
                    sail_date,
                )
            })
            .await?;
        if cats.is_empty() {
            return Ok(PublicFetch::PlannerNotOpen);
        }

        let mut products = Vec::new();
        for cat in &cats {
            self.limiter.wait().await;
            let prods = self
                .retry(|| {
                    fetch_products_in_category(
                        &self.http,
                        &self.cfg.app_key,
                        &self.cfg.user_agent,
                        brand,
                        ship_code,
                        sail_date,
                        &cat.id,
                        None, // no passenger — public path
                        None, // OMIT reservationId — the load-bearing fix
                        "USD",
                    )
                })
                .await?;
            for p in prods {
                let price = p.first_price();
                let promo_cents = price.and_then(|pr| {
                    pr.formatted_promotional_price
                        .as_deref()
                        .or(pr.promotional_price.as_deref())
                        .and_then(parse_money_cents)
                });
                let base_cents = price.and_then(|pr| {
                    pr.formatted_base_price
                        .as_deref()
                        .or(pr.shipboard_price.as_deref())
                        .and_then(parse_money_cents)
                });
                products.push(PublicProduct {
                    category_id: cat.id.clone(),
                    category_name: cat.name.clone(),
                    product_code: p.id.clone(),
                    title: p.title.clone(),
                    promo_cents,
                    base_cents,
                    promo_label: price.and_then(|pr| pr.best_price_label()),
                    base_label: price.and_then(|pr| pr.best_base_label()),
                    currency: price.and_then(|pr| pr.currency.clone()),
                    unit_label: price.and_then(|pr| {
                        pr.sales_unit.as_ref().and_then(|u| u.label.clone())
                    }),
                });
            }
        }
        if products.is_empty() {
            return Ok(PublicFetch::PlannerNotOpen);
        }
        Ok(PublicFetch::Products(products))
    }
}

impl PublicClientConfig {
    fn base_backoff_pow(&self, attempt: u32) -> Duration {
        let mult = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
        (self.pacing.base_backoff * mult).min(self.pacing.max_backoff)
    }
}
