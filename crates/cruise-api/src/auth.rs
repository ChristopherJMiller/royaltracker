use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use crate::error::ApiError;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub(crate) struct OAuthTokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TokenState {
    pub access_token: String,
    pub account_id: String,
    pub expires_at: DateTime<Utc>,
}

impl TokenState {
    pub fn is_expired(&self, skew: Duration) -> bool {
        Utc::now() + skew >= self.expires_at
    }
}

#[derive(Debug, Deserialize)]
struct JwtClaims {
    #[serde(default)]
    sub: Option<String>,
}

/// Extract the `sub` claim (accountId) from the JWT without verifying the signature.
/// We trust the issuer's response — we just need the user id it told us.
pub fn decode_account_id(jwt: &str) -> Result<String, ApiError> {
    let mut parts = jwt.split('.');
    let _header = parts.next().ok_or(ApiError::BadJwt)?;
    let payload = parts.next().ok_or(ApiError::BadJwt)?;

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload.as_bytes()))?;

    let claims: JwtClaims = serde_json::from_slice(&decoded)?;
    claims.sub.ok_or(ApiError::BadJwt)
}

pub(crate) fn build_token_state(resp: OAuthTokenResponse) -> Result<TokenState, ApiError> {
    let account_id = decode_account_id(&resp.access_token)?;
    let ttl = resp.expires_in.unwrap_or(3600);
    Ok(TokenState {
        access_token: resp.access_token,
        account_id,
        expires_at: Utc::now() + Duration::seconds(ttl),
    })
}
