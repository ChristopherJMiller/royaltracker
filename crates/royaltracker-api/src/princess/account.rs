//! Princess account client: guest-site login and current-booking context
//! (spec §9.1 steps 1-4). One login attempt per call, never retried. The
//! `booking/display/menu` tier call is deliberately not made (prices are
//! tier-agnostic, spec §5.4).

use base64::Engine;
use chrono::NaiveDate;
use rand::RngCore;

use super::PCL_CLIENT_ID;
use crate::client::{build_emulated_client, warm_up_host, DEFAULT_USER_AGENT};
use crate::error::ApiError;

const WWW_HOST: &str = "www.princess.com";
const ORIGIN: &str = "https://www.princess.com";
const LOGIN_URL: &str = "https://gw.api.princess.com/pcl-web/internal/guest/p1.0/login";
const QUICK_LOGIN_URL: &str = "https://book.princess.com/captaincircle/quickLogin.do";
const MANAGE_BOOKING_URL: &str = "https://book.princess.com/manage-booking/index.page";
const MAX_REDIRECTS: usize = 5;
const KEEP_ALIVE_URL: &str = "https://book.princess.com/manage-booking/json/sfccKeepAlive.page";
const ERROR_BODY_MAX: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrincessBookingContext {
    pub booking_id: String,
    pub ship_code: String,
    pub voyage_code: String,
    pub pax_id: String,
    pub nights: Option<i32>,
    pub sail_date: NaiveDate,
    pub currency: String,
}

/// `"Basic " + base64(urlencode(login) + ":" + urlencode(password))`.
/// Spaces are encoded as `%20` to match the site's `encodeURIComponent` (form
/// encoding would emit `+`; a literal `+` is already `%2B`).
pub fn basic_auth_value(login: &str, password: &str) -> String {
    let enc = |s: &str| {
        url::form_urlencoded::byte_serialize(s.as_bytes())
            .collect::<String>()
            .replace('+', "%20")
    };
    let raw = format!("{}:{}", enc(login), enc(password));
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
}

/// Parse `"Saturday, October 24, 2026"`.
pub fn parse_departure(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%A, %B %d, %Y").ok()
}

/// Extract the current booking from an `sfccKeepAlive` response. `None` when
/// `data.bookingInfo` is absent, or any required field (`bookingId`,
/// `shipCode`, `voyageId`, `paxId`, `departureDate`, `bookingCurrency`) is
/// missing or empty, or `departureDate` doesn't parse. `durationDays` is
/// optional. When `bookingId` is present but parsing fails, the failing field
/// NAME (never its value) is logged.
pub fn parse_keep_alive(v: &serde_json::Value) -> Option<PrincessBookingContext> {
    let bi = v.get("data")?.get("bookingInfo")?;
    let text = |k: &str| match bi.get(k)? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    let booking_id = text("bookingId")?;
    let required = |k: &'static str| {
        let r = text(k);
        if r.is_none() {
            tracing::warn!(field = k, "princess keepalive bookingInfo field missing or empty");
        }
        r
    };
    let ship_code = required("shipCode")?;
    let voyage_code = required("voyageId")?;
    let pax_id = required("paxId")?;
    let departure = required("departureDate")?;
    let Some(sail_date) = parse_departure(&departure) else {
        tracing::warn!(field = "departureDate", "princess keepalive bookingInfo field unparseable");
        return None;
    };
    let currency = required("bookingCurrency")?;
    let nights = text("durationDays").and_then(|d| d.parse::<i32>().ok());
    Some(PrincessBookingContext {
        booking_id,
        ship_code,
        voyage_code,
        pax_id,
        nights,
        sail_date,
        currency,
    })
}

/// Require a JSON body whose top-level `status` is `"success"`; otherwise
/// `ApiError::Status` with `http_status` and the body truncated to 200 chars.
fn require_success(http_status: u16, body: &str) -> Result<serde_json::Value, ApiError> {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(v) if v.get("status").and_then(|s| s.as_str()) == Some("success") => Ok(v),
        _ => Err(ApiError::Status {
            status: http_status,
            body: error_snippet(body),
        }),
    }
}

/// Error-message body: at most 200 chars, with any JSON key whose name
/// contains "token" removed first (e.g. keepalive's `tokenObj.appToken`), so
/// a session token never reaches stored errors or logs.
fn error_snippet(body: &str) -> String {
    let cleaned = match serde_json::from_str::<serde_json::Value>(body) {
        Ok(mut v) => {
            if strip_token_keys(&mut v) {
                v.to_string()
            } else {
                body.to_string()
            }
        }
        Err(_) => body.to_string(),
    };
    cleaned.chars().take(ERROR_BODY_MAX).collect()
}

/// Recursively drops keys containing "token" (case-insensitive). Returns
/// whether anything was removed.
fn strip_token_keys(v: &mut serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(m) => {
            let before = m.len();
            m.retain(|k, _| !k.to_ascii_lowercase().contains("token"));
            let mut removed = m.len() != before;
            for x in m.values_mut() {
                removed |= strip_token_keys(x);
            }
            removed
        }
        serde_json::Value::Array(a) => a.iter_mut().fold(false, |acc, x| strip_token_keys(x) | acc),
        _ => false,
    }
}

pub struct PrincessAccountClient {
    http: wreq::Client,
}

impl PrincessAccountClient {
    pub fn new() -> Result<Self, ApiError> {
        Ok(Self { http: build_emulated_client()? })
    }

    /// Exactly one login attempt (no retry), then bridge into the booking site
    /// and read the current booking. `Ok(None)` when the account has no
    /// current booking.
    pub async fn login_and_context(
        &self,
        login: &str,
        password: &str,
    ) -> Result<Option<PrincessBookingContext>, ApiError> {
        warm_up_host(&self.http, DEFAULT_USER_AGENT, WWW_HOST).await?;

        let app_id = serde_json::json!({
            "env": "prod",
            "migration": "N",
            "sessionId": uuid4(),
        })
        .to_string();
        let resp = self
            .http
            .post(LOGIN_URL)
            .header("User-Agent", DEFAULT_USER_AGENT)
            .header("Accept", "*/*")
            .header("Content-Type", "application/json")
            .header("BookingCompany", "PC")
            .header("ProductCompany", "PC")
            .header("ReqSrc", "W")
            .header("pcl-client-id", PCL_CLIENT_ID)
            .header("Origin", ORIGIN)
            .header("Referer", "https://www.princess.com/")
            .header("AppId", app_id)
            .header("Authorization", basic_auth_value(login, password))
            .body("")
            .send()
            .await?;
        check_ok(resp).await?;

        let epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let resp = self
            .http
            .get(format!("{QUICK_LOGIN_URL}?temporary={epoch_ms}"))
            .header("User-Agent", DEFAULT_USER_AGENT)
            .header("Accept", "*/*")
            .header("Origin", ORIGIN)
            .header("Referer", "https://www.princess.com/")
            .send()
            .await?;
        check_success(resp).await?;

        // Loading the booking site's landing page (302 -> summary.page) is what
        // sets the session's "current booking"; without it sfccKeepAlive has no
        // bookingInfo.
        let resp = self
            .http
            .get(MANAGE_BOOKING_URL)
            .header("User-Agent", DEFAULT_USER_AGENT)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Referer", "https://www.princess.com/")
            .redirect(wreq::redirect::Policy::limited(MAX_REDIRECTS))
            .send()
            .await?;
        check_ok(resp).await?;

        let resp = self
            .http
            .get(KEEP_ALIVE_URL)
            .header("User-Agent", DEFAULT_USER_AGENT)
            .header("Accept", "*/*")
            .header("Content-Type", "application/json")
            .header("bookingcompany", "PC")
            .header("productcompany", "PC")
            .header("Origin", ORIGIN)
            .send()
            .await?;
        let v = check_success(resp).await?;
        Ok(parse_keep_alive(&v))
    }
}

/// [`check_ok`], then require a JSON body with top-level `"status": "success"`.
async fn check_success(resp: wreq::Response) -> Result<serde_json::Value, ApiError> {
    let status = resp.status().as_u16();
    let body = check_ok(resp).await?;
    require_success(status, &body)
}

/// Return the body of a 2xx response; otherwise `ApiError::Status` with the
/// body truncated to 200 chars. Never includes request headers.
async fn check_ok(resp: wreq::Response) -> Result<String, ApiError> {
    let status = resp.status();
    let body = resp.text().await?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(ApiError::Status {
            status: status.as_u16(),
            body: error_snippet(&body),
        })
    }
}

fn uuid4() -> String {
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid4_shape() {
        let u = super::uuid4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }

    #[test]
    fn require_success_accepts_only_status_success() {
        let v = require_success(200, r#"{"status":"success","data":{}}"#).unwrap();
        assert!(v.get("data").is_some());

        for body in [r#"{"status":"failure","msg":"x"}"#, r#"{"data":{}}"#, "<html>login</html>", ""] {
            match require_success(200, body) {
                Err(ApiError::Status { status, body: b }) => {
                    assert_eq!(status, 200);
                    assert_eq!(b, body);
                }
                other => panic!("expected Status error for {body:?}, got {other:?}"),
            }
        }

        let long = format!(r#"{{"status":"error","pad":"{}"}}"#, "x".repeat(500));
        match require_success(200, &long) {
            Err(ApiError::Status { body, .. }) => assert_eq!(body.chars().count(), ERROR_BODY_MAX),
            other => panic!("expected Status error, got {other:?}"),
        }
    }

    #[test]
    fn error_snippet_never_carries_tokens() {
        let body = r#"{"data":{"httpcode":401,"tokenObj":{"appToken":"SECRET","expiresIn":30},"msg":"no session"},"status":"error"}"#;
        match require_success(200, body) {
            Err(ApiError::Status { body: b, .. }) => {
                assert!(!b.contains("SECRET") && !b.contains("tokenObj"), "{b}");
                assert!(b.contains("no session") && b.contains("\"status\":\"error\""), "{b}");
            }
            other => panic!("expected Status error, got {other:?}"),
        }
        assert!(!error_snippet(r#"[{"Token":"SECRET"}]"#).contains("SECRET"));
        assert_eq!(error_snippet("<html>x</html>"), "<html>x</html>");
    }
}
