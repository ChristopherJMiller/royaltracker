//! Web Push (VAPID) delivery via `web-push-native` (pure RustCrypto: aes-gcm +
//! ece-native + p256 + jwt-simple — no openssl, so no BoringSSL symbol clash).
//! The crate builds a signed, aes128gcm-encrypted `http::Request`; we send it
//! with reqwest (rustls). A 404/410 means the endpoint is dead.

use crate::{NotifyError, PriceDropAlert, PushSubscription};
use base64::Engine;
use web_push_native::{jwt_simple::algorithms::ES256KeyPair, p256::PublicKey, Auth, WebPushBuilder};

pub struct WebPushChannel {
    http: reqwest::Client,
    key_pair: ES256KeyPair,
    /// VAPID `sub` claim, e.g. `mailto:you@example.com`.
    subject: String,
}

fn b64url(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s.trim_end_matches('='))
}

impl WebPushChannel {
    /// `vapid_private_key_b64` is the base64url-encoded raw P-256 private scalar
    /// (what standard VAPID tooling emits).
    pub fn new(vapid_private_key_b64: String, subject: String) -> Result<Self, String> {
        let raw = b64url(&vapid_private_key_b64).map_err(|e| format!("vapid key b64: {e}"))?;
        let key_pair = ES256KeyPair::from_bytes(&raw).map_err(|e| format!("vapid key: {e}"))?;
        Ok(Self {
            http: reqwest::Client::new(),
            key_pair,
            subject,
        })
    }

    pub(crate) async fn send_price_drop(
        &self,
        sub: &PushSubscription,
        alert: &PriceDropAlert<'_>,
    ) -> Result<(), NotifyError> {
        let endpoint: http::Uri = sub
            .endpoint
            .parse()
            .map_err(|_| NotifyError::Permanent("bad push endpoint".into()))?;
        let p256dh =
            b64url(&sub.p256dh).map_err(|_| NotifyError::Permanent("bad p256dh".into()))?;
        let auth = b64url(&sub.auth).map_err(|_| NotifyError::Permanent("bad auth".into()))?;
        if auth.len() != 16 {
            return Err(NotifyError::Permanent("bad auth length".into()));
        }
        let ua_public = PublicKey::from_sec1_bytes(&p256dh)
            .map_err(|_| NotifyError::Permanent("bad p256 public key".into()))?;
        let ua_auth = *Auth::from_slice(&auth);

        let body = format!(
            "${:.2} — was ${:.2} ({:+.1}%){}",
            alert.diff.new_price,
            alert.diff.old_price,
            alert.diff.delta_pct,
            alert.itinerary.map(|i| format!("\n{i}")).unwrap_or_default()
        );
        let payload = serde_json::json!({
            "title": format!("🔻 {}", alert.label),
            "body": body,
            "url": alert.manage_url,
        })
        .to_string();

        let request = WebPushBuilder::new(endpoint, ua_public, ua_auth)
            .with_vapid(&self.key_pair, &self.subject)
            .build(payload.into_bytes())
            .map_err(|e| NotifyError::Permanent(format!("push build: {e}")))?;

        // web-push-native builds an http 0.2 request; reqwest is on http 1.x, so
        // translate through reqwest's builder rather than a cross-version TryFrom.
        let (parts, body) = request.into_parts();
        let mut rb = self.http.post(parts.uri.to_string());
        for (name, value) in parts.headers.iter() {
            rb = rb.header(name.as_str(), value.as_bytes());
        }
        let resp = rb
            .body(body)
            .send()
            .await
            .map_err(|e| NotifyError::Transient(e.to_string()))?;
        match resp.status().as_u16() {
            200..=299 => Ok(()),
            404 | 410 => Err(NotifyError::SubscriptionGone),
            429 | 500..=599 => Err(NotifyError::Transient(format!("push status {}", resp.status()))),
            other => Err(NotifyError::Permanent(format!("push status {other}"))),
        }
    }
}
