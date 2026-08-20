//! Anonymous device identity for the public tier: a signed cookie (no PII, no
//! account) that groups a browser's subscriptions, plus Cloudflare Turnstile
//! verification for the subscribe action.

use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;
const COOKIE_NAME: &str = "rt_dev";

/// A fresh random device id (128 bits, hex).
pub fn mint_device_id() -> String {
    let b: [u8; 16] = rand::random();
    hex::encode(b)
}

fn sign(device_id: &str, key: &[u8]) -> String {
    let mut m = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    m.update(device_id.as_bytes());
    hex::encode(m.finalize().into_bytes())
}

/// `Set-Cookie` header value binding a browser to `device_id`.
pub fn set_cookie_header(device_id: &str, key: &[u8]) -> String {
    format!(
        "{COOKIE_NAME}={}.{}; HttpOnly; Secure; SameSite=Lax; Path=/public; Max-Age=31536000",
        device_id,
        sign(device_id, key)
    )
}

/// Extract and verify the device id from the Cookie header, if present and valid.
pub fn read_device(headers: &HeaderMap, key: &[u8]) -> Option<String> {
    let cookie = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{COOKIE_NAME}=");
    for part in cookie.split(';') {
        let p = part.trim();
        if let Some(v) = p.strip_prefix(&prefix) {
            let (id, sig) = v.split_once('.')?;
            if constant_time_eq(sig.as_bytes(), sign(id, key).as_bytes()) {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        d |= x ^ y;
    }
    d == 0
}

/// Verify a Turnstile token server-side. Fails closed on any error.
pub async fn verify_turnstile(
    http: &reqwest::Client,
    secret: &str,
    token: &str,
    remoteip: Option<&str>,
) -> bool {
    #[derive(serde::Deserialize)]
    struct SiteVerify {
        success: bool,
    }
    let mut form = vec![("secret", secret), ("response", token)];
    if let Some(ip) = remoteip {
        form.push(("remoteip", ip));
    }
    match http
        .post("https://challenges.cloudflare.com/turnstile/v0/siteverify")
        .form(&form)
        .send()
        .await
    {
        Ok(r) => r.json::<SiteVerify>().await.map(|x| x.success).unwrap_or(false),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie_from(header_val: &str) -> HeaderMap {
        // set_cookie_header returns "rt_dev=<v>; HttpOnly; ..." — take the first pair.
        let pair = header_val.split(';').next().unwrap().trim();
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::COOKIE, pair.parse().unwrap());
        h
    }

    #[test]
    fn device_cookie_roundtrip() {
        let key = b"0123456789abcdef-secret-key";
        let id = mint_device_id();
        let headers = cookie_from(&set_cookie_header(&id, key));
        assert_eq!(read_device(&headers, key).as_deref(), Some(id.as_str()));
    }

    #[test]
    fn device_cookie_rejects_tamper_and_wrong_key() {
        let key = b"the-real-key-aaaaaaaa";
        let id = mint_device_id();
        let good = set_cookie_header(&id, key);
        // Wrong key → reject.
        assert_eq!(read_device(&cookie_from(&good), b"a-different-key-bbbb"), None);
        // Forged signature → reject.
        let mut forged = HeaderMap::new();
        forged.insert(
            axum::http::header::COOKIE,
            "rt_dev=deadbeefdeadbeef.0000".parse().unwrap(),
        );
        assert_eq!(read_device(&forged, key), None);
    }
}
