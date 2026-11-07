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

        let payload = serde_json::json!({
            "title": push_title(alert),
            "body": push_body(alert),
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

fn push_title(alert: &PriceDropAlert<'_>) -> String {
    match alert.paid {
        Some(_) => format!("🔻 {} is cheaper than you paid", alert.label),
        None => format!("🔻 {}", alert.label),
    }
}

/// Notification body: new price (+ unit), old price, % change, itinerary. For
/// a booked item, today's price against what was paid and the savings.
fn push_body(alert: &PriceDropAlert<'_>) -> String {
    let itinerary = alert.itinerary.map(|i| format!("\n{i}")).unwrap_or_default();
    if let Some(p) = &alert.paid {
        let unit = p.unit.map(|u| format!(" {u}")).unwrap_or_default();
        let group = p.group.map(|n| format!(" for {n}")).unwrap_or_default();
        let everyone = if p.group.is_some() { " everyone" } else { "" };
        return format!(
            "{}{unit}{group} — you paid {}\nRebook{everyone} to save {}{itinerary}",
            p.now_label, p.paid_label, p.savings_label
        );
    }
    let unit = alert
        .unit_label
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(|u| format!(" {u}"))
        .unwrap_or_default();
    format!(
        "{}{unit} — was {} ({:+.1}%){}",
        royaltracker_types::format_money(alert.diff.new_price, alert.currency),
        royaltracker_types::format_money(alert.diff.old_price, alert.currency),
        alert.diff.delta_pct,
        itinerary
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use royaltracker_types::Diff;

    fn alert<'a>(diff: &'a Diff, currency: Option<&'a str>, unit: Option<&'a str>) -> PriceDropAlert<'a> {
        PriceDropAlert {
            label: "Classic Soda",
            diff,
            msrp_label: None,
            itinerary: Some("Ship · Jan 2, 2027"),
            manage_url: None,
            currency,
            unit_label: unit,
            paid: None,
        }
    }

    #[test]
    fn push_body_without_unit_unchanged() {
        let d = Diff::from_prices(1, 30.0, 24.94);
        assert_eq!(push_body(&alert(&d, None, None)), "$24.94 — was $30.00 (-16.9%)\nShip · Jan 2, 2027");
    }

    #[test]
    fn push_body_shows_unit_after_new_price() {
        let d = Diff::from_prices(1, 30.0, 24.94);
        assert_eq!(
            push_body(&alert(&d, Some("AUD"), Some("per person / day"))),
            "AUD 24.94 per person / day — was AUD 30.00 (-16.9%)\nShip · Jan 2, 2027"
        );
    }
}

#[cfg(test)]
mod paid_tests {
    use super::*;
    use crate::PaidComparison;
    use royaltracker_types::Diff;

    #[test]
    fn paid_push_title_and_body() {
        let d = Diff::from_prices(1, 145.98, 137.98);
        let alert = PriceDropAlert {
            label: "Deluxe Beverage Package",
            diff: &d,
            msrp_label: None,
            itinerary: Some("Harmony of the Seas · 7 nights · Mar 14, 2027"),
            manage_url: None,
            currency: None,
            unit_label: None,
            paid: Some(PaidComparison {
                now_label: "$68.99",
                paid_label: "$72.99",
                unit: Some("per day"),
                group: None,
                savings_label: "$56.00",
                breakdown: Some("2 guests × 7 days × $4.00"),
            }),
        };
        assert_eq!(push_title(&alert), "🔻 Deluxe Beverage Package is cheaper than you paid");
        assert_eq!(
            push_body(&alert),
            "$68.99 per day — you paid $72.99\nRebook to save $56.00\nHarmony of the Seas · 7 nights · Mar 14, 2027"
        );
    }

    #[test]
    fn paid_push_body_for_a_group() {
        let d = Diff::from_prices(1, 145.98, 137.98);
        let alert = PriceDropAlert {
            label: "Deluxe Beverage Package",
            diff: &d,
            msrp_label: None,
            itinerary: None,
            manage_url: None,
            currency: None,
            unit_label: None,
            paid: Some(PaidComparison {
                now_label: "$137.98",
                paid_label: "$145.98",
                unit: Some("per day"),
                group: Some(2),
                savings_label: "$56.00",
                breakdown: None,
            }),
        };
        assert_eq!(push_body(&alert), "$137.98 per day for 2 — you paid $145.98\nRebook everyone to save $56.00");
    }
}
