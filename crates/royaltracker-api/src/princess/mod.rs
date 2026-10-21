//! Princess Cruises guest (no-login) price client over Salesforce Commerce
//! Cloud. Guest token only; no shopper context is ever set (prices are
//! identical with and without it). See spec §9.2.

mod catalog;

pub use catalog::{apply_product_details, category_name, parse_search_hits};

use base64::Engine;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::client::{build_emulated_client, DEFAULT_USER_AGENT};
use crate::error::ApiError;
use crate::public::{is_bot_challenge, retry_with_pacing, PacingConfig, PublicProduct, RateLimiter};

pub const PCL_CLIENT_ID: &str = "32e7224ac6cc41302f673c5f5d27b4ba";
pub const SLAS_CLIENT_ID: &str = "25ddb611-56bf-4c76-85e1-c7c83c412085";
pub const SLAS_PLACEHOLDER_SECRET: &str = "_PLACEHOLDER_PROXY-PWA_KIT_SLAS_CLIENT_SECRET";
pub const SFCC_ORG: &str = "f_ecom_blnk_prd";
pub const SFCC_SITE: &str = "PCL";
pub const EXTRAS_CATEGORIES: [&str; 3] = ["food-and-drinks", "wifi", "extras"];

const BASE: &str = "https://www.princess.com/mobify";
const ORIGIN: &str = "https://www.princess.com";
const PAGE_LIMIT: usize = 200;
const DETAIL_BATCH: usize = 24;
/// Refresh the token this long before it expires.
const TOKEN_SKEW: Duration = Duration::from_secs(60);

struct GuestToken {
    access_token: String,
    refresh_at: Instant,
}

pub struct PrincessGuestClient {
    http: wreq::Client,
    pacing: PacingConfig,
    limiter: RateLimiter,
    token: Mutex<Option<GuestToken>>,
}

impl PrincessGuestClient {
    pub fn new(pacing: PacingConfig) -> Result<Self, ApiError> {
        Ok(Self {
            http: build_emulated_client()?,
            limiter: RateLimiter::new(pacing.clone()),
            pacing,
            token: Mutex::new(None),
        })
    }

    /// Excursions for one voyage, priced in `currency`.
    pub async fn fetch_excursions(
        &self,
        voyage_code: &str,
        currency: &str,
    ) -> Result<Vec<PublicProduct>, ApiError> {
        let refine = vec![
            "cgid=excursions".to_string(),
            format!("ilids={voyage_code}"),
            "htype=master|bundle|set".to_string(),
            "price=(0..)".to_string(),
        ];
        self.search_all(&refine, "excursions", currency).await
    }

    /// Fleet-wide extras (food-and-drinks, wifi, extras) for one currency, with
    /// units filled from the product detail endpoint.
    pub async fn fetch_extras(&self, currency: &str) -> Result<Vec<PublicProduct>, ApiError> {
        let mut all = Vec::new();
        for cat in EXTRAS_CATEGORIES {
            let refine = vec![
                format!("cgid={cat}"),
                "ilids=pcl-extras-inventory".to_string(),
                "price=(0..)".to_string(),
            ];
            all.extend(self.search_all(&refine, cat, currency).await?);
        }
        let ids: Vec<String> = all.iter().map(|p| p.product_code.clone()).collect();
        for chunk in ids.chunks(DETAIL_BATCH) {
            let url = format!(
                "{BASE}/proxy/api/product/shopper-products/v1/organizations/{SFCC_ORG}/products?{}",
                query(&[
                    ("siteId", SFCC_SITE),
                    ("ids", &chunk.join(",")),
                    ("expand", "prices,promotions"),
                    ("currency", currency),
                    ("locale", "en-US"),
                    ("allImages", "false"),
                ])
            );
            let v = self.get_json(&url).await?;
            apply_product_details(&mut all, &v);
        }
        Ok(all)
    }

    async fn search_all(
        &self,
        refine: &[String],
        category_id: &str,
        currency: &str,
    ) -> Result<Vec<PublicProduct>, ApiError> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        loop {
            let mut params: Vec<(&str, String)> = vec![
                ("siteId", SFCC_SITE.into()),
                ("currency", currency.into()),
                ("locale", "en-US".into()),
                ("expand", "prices,promotions,represented_products".into()),
                ("offset", offset.to_string()),
                ("limit", PAGE_LIMIT.to_string()),
            ];
            for r in refine {
                params.push(("refine", r.clone()));
            }
            let pairs: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let url = format!(
                "{BASE}/proxy/api/search/shopper-search/v1/organizations/{SFCC_ORG}/product-search?{}",
                query(&pairs)
            );
            let v = self.get_json(&url).await?;
            let page = parse_search_hits(&v, category_id, currency);
            let got = v
                .get("hits")
                .and_then(|h| h.as_array())
                .map_or(0, |h| h.len());
            let total = v.get("total").and_then(|t| t.as_u64()).unwrap_or(0) as usize;
            if v.get("total").is_none() && got >= PAGE_LIMIT {
                tracing::warn!(category_id, offset, "princess search page full but `total` absent");
            }
            out.extend(page);
            offset += got;
            if got == 0 || offset >= total {
                break;
            }
        }
        Ok(out)
    }


    async fn get_json(&self, url: &str) -> Result<serde_json::Value, ApiError> {
        let text = retry_with_pacing(&self.pacing, || async {
            let mut reauthed = false;
            loop {
                let token = self.bearer().await?;
                self.limiter.wait().await;
                let resp = self
                    .http
                    .get(url)
                    .header("User-Agent", DEFAULT_USER_AGENT)
                    .header("Accept", "application/json")
                    .header("Origin", ORIGIN)
                    .header("Authorization", format!("Bearer {token}"))
                    .send()
                    .await?;
                let status = resp.status();
                let body = resp.text().await?;
                if should_retry_after_auth_failure(status.as_u16(), reauthed) {
                    // Token rejected (e.g. revoked): drop it and retry this
                    // request once with a freshly minted token.
                    *self.token.lock().await = None;
                    reauthed = true;
                    continue;
                }
                if !status.is_success() || is_bot_challenge(&body) {
                    return Err(ApiError::Status {
                        status: status.as_u16(),
                        body,
                    });
                }
                return Ok(body);
            }
        })
        .await?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Lazily mint a guest token; reuse it until 60 s before it expires.
    async fn bearer(&self) -> Result<String, ApiError> {
        let mut guard = self.token.lock().await;
        if let Some(t) = guard.as_ref() {
            if Instant::now() < t.refresh_at {
                return Ok(t.access_token.clone());
            }
        }
        self.limiter.wait().await;
        let basic = base64::engine::general_purpose::STANDARD
            .encode(format!("{SLAS_CLIENT_ID}:{SLAS_PLACEHOLDER_SECRET}"));
        let url = format!("{BASE}/slas/private/shopper/auth/v1/organizations/{SFCC_ORG}/oauth2/token");
        // Minimal header set: extra browser-ish headers made this flaky.
        let resp = self
            .http
            .post(&url)
            .header("User-Agent", DEFAULT_USER_AGENT)
            .header("Origin", ORIGIN)
            .header("Authorization", format!("Basic {basic}"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body("grant_type=client_credentials&channel_id=PCL&dnt=false")
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            // Never echo a token-endpoint body beyond what is needed for challenge detection.
            return Err(ApiError::Status {
                status: status.as_u16(),
                body: if is_bot_challenge(&body) { body } else { String::new() },
            });
        }
        let v: serde_json::Value = serde_json::from_str(&body)?;
        let access_token = v
            .get("access_token")
            .and_then(|t| t.as_str())
            .ok_or(ApiError::MissingField("access_token"))?
            .to_string();
        let ttl = v.get("expires_in").and_then(|e| e.as_u64()).unwrap_or(1800);
        let refresh_at = Instant::now() + Duration::from_secs(ttl).saturating_sub(TOKEN_SKEW);
        *guard = Some(GuestToken {
            access_token: access_token.clone(),
            refresh_at,
        });
        Ok(access_token)
    }
}

fn query(pairs: &[(&str, &str)]) -> String {
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        s.append_pair(k, v);
    }
    s.finish()
}

/// A 401 on an SFCC call means the cached guest token was rejected; retry once
/// with a fresh one, never more (RCG `is_retryable` is deliberately untouched).
fn should_retry_after_auth_failure(status: u16, already_retried: bool) -> bool {
    status == 401 && !already_retried
}

#[cfg(test)]
mod tests {
    use super::should_retry_after_auth_failure as f;

    #[test]
    fn retries_401_exactly_once() {
        assert!(f(401, false));
        assert!(!f(401, true));
        assert!(!f(200, false));
        assert!(!f(403, false));
        assert!(!f(500, false));
    }
}
