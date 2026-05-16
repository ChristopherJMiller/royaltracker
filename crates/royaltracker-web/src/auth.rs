use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use royaltracker_storage::PriceRepo;
use royaltracker_types::User;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use std::sync::Arc;

use crate::state::AppState;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing X-Telegram-Init-Data header")]
    Missing,
    #[error("malformed initData: {0}")]
    Malformed(&'static str),
    #[error("HMAC mismatch (initData was not signed by this bot)")]
    BadHmac,
    #[error("initData expired (auth_date is older than the window)")]
    Expired,
    #[error("user not registered with this bot")]
    Unregistered,
    #[error("storage: {0}")]
    Storage(#[from] royaltracker_storage::StorageError),
}

impl axum::response::IntoResponse for AuthError {
    fn into_response(self) -> axum::response::Response {
        let code = match self {
            AuthError::Missing | AuthError::Malformed(_) => StatusCode::BAD_REQUEST,
            AuthError::BadHmac | AuthError::Expired => StatusCode::UNAUTHORIZED,
            AuthError::Unregistered => StatusCode::FORBIDDEN,
            AuthError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (code, self.to_string()).into_response()
    }
}

/// Telegram WebApp `initData` is a `key=value&key=value&...` query string.
/// Verification per https://core.telegram.org/bots/webapps#validating-data-received-via-the-mini-app
///   1. Parse pairs
///   2. Pull out `hash`, sort remaining keys lex
///   3. data_check_string = "key=value\nkey=value\n..."
///   4. secret_key = HMAC-SHA256(message=bot_token, key="WebAppData")
///   5. expected_hash = HMAC-SHA256(message=data_check_string, key=secret_key)
///   6. Compare hex(expected_hash) to provided hash (constant time)
pub fn verify_init_data(raw: &str, bot_token: &str) -> Result<TgUser, AuthError> {
    let mut pairs: Vec<(String, String)> = url::form_urlencoded::parse(raw.as_bytes())
        .into_owned()
        .collect();

    let hash_idx = pairs.iter().position(|(k, _)| k == "hash")
        .ok_or(AuthError::Malformed("no hash"))?;
    let (_, provided_hash) = pairs.remove(hash_idx);

    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let data_check_string = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut mac = HmacSha256::new_from_slice(b"WebAppData")
        .map_err(|_| AuthError::Malformed("hmac key"))?;
    mac.update(bot_token.as_bytes());
    let secret_key = mac.finalize().into_bytes();

    let mut mac = HmacSha256::new_from_slice(&secret_key)
        .map_err(|_| AuthError::Malformed("hmac key 2"))?;
    mac.update(data_check_string.as_bytes());
    let computed = mac.finalize().into_bytes();
    let computed_hex = hex::encode(computed);

    if !constant_time_eq(computed_hex.as_bytes(), provided_hash.as_bytes()) {
        return Err(AuthError::BadHmac);
    }

    // Freshness check — Telegram's recommendation is < 24h.
    if let Some((_, auth_date)) = pairs.iter().find(|(k, _)| k == "auth_date") {
        if let Ok(ts) = auth_date.parse::<i64>() {
            let now = chrono::Utc::now().timestamp();
            if now - ts > 60 * 60 * 24 {
                return Err(AuthError::Expired);
            }
        }
    }

    let user_json = pairs.iter().find(|(k, _)| k == "user")
        .map(|(_, v)| v.as_str())
        .ok_or(AuthError::Malformed("no user field"))?;

    serde_json::from_str::<TgUser>(user_json)
        .map_err(|_| AuthError::Malformed("user json"))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Debug, Clone, Deserialize)]
pub struct TgUser {
    pub id: i64,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub language_code: Option<String>,
}

/// Extractor that validates the X-Telegram-Init-Data header and resolves to the
/// app's registered User. Use as a handler argument.
pub struct AuthedUser {
    pub tg: TgUser,
    pub db_user: User,
}

#[async_trait]
impl FromRequestParts<Arc<AppState>> for AuthedUser {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let tg = TgAuthOnly::from_request_parts(parts, state).await?.tg;
        let db_user = state
            .repo
            .get_user_by_chat_id(tg.id)
            .await?
            .ok_or(AuthError::Unregistered)?;
        Ok(Self { tg, db_user })
    }
}

/// Like AuthedUser but does NOT require the user to be registered yet.
/// Use for endpoints that create a User row (e.g. /api/register).
pub struct TgAuthOnly {
    pub tg: TgUser,
}

#[async_trait]
impl FromRequestParts<Arc<AppState>> for TgAuthOnly {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let raw = parts
            .headers
            .get("x-telegram-init-data")
            .ok_or(AuthError::Missing)?
            .to_str()
            .map_err(|_| AuthError::Malformed("non-ascii header"))?
            .to_owned();
        let tg = verify_init_data(&raw, &state.bot_token)?;
        Ok(Self { tg })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip: build a fake initData with a known token, then verify it.
    #[test]
    fn verify_round_trip() {
        let bot_token = "1234:abcdef";
        let user = r#"{"id":42,"first_name":"Test"}"#;
        let auth_date = chrono::Utc::now().timestamp().to_string();
        let mut pairs = vec![
            ("auth_date".to_string(), auth_date.clone()),
            ("user".to_string(), user.to_string()),
        ];
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let dcs = pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut mac = HmacSha256::new_from_slice(b"WebAppData").unwrap();
        mac.update(bot_token.as_bytes());
        let secret_key = mac.finalize().into_bytes();
        let mut mac = HmacSha256::new_from_slice(&secret_key).unwrap();
        mac.update(dcs.as_bytes());
        let hash = hex::encode(mac.finalize().into_bytes());

        let raw = format!(
            "auth_date={auth_date}&user={}&hash={hash}",
            url::form_urlencoded::byte_serialize(user.as_bytes()).collect::<String>(),
        );
        let parsed = verify_init_data(&raw, bot_token).unwrap();
        assert_eq!(parsed.id, 42);
    }

    #[test]
    fn rejects_tampered_hash() {
        let raw = "auth_date=1700000000&user=%7B%22id%22%3A1%7D&hash=00000000";
        assert!(matches!(
            verify_init_data(raw, "1234:abc"),
            Err(AuthError::BadHmac)
        ));
    }
}
