//! Princess account client: guest-site login and current-booking context
//! (spec §9.1 steps 1-3). One login attempt per call, never retried. The
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
pub fn basic_auth_value(login: &str, password: &str) -> String {
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let raw = format!("{}:{}", enc(login), enc(password));
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
}

/// Parse `"Saturday, October 24, 2026"`.
pub fn parse_departure(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%A, %B %d, %Y").ok()
}

/// Extract the current booking from an `sfccKeepAlive` response. `None` when
/// `data.bookingInfo` is absent or lacks `bookingId`.
pub fn parse_keep_alive(v: &serde_json::Value) -> Option<PrincessBookingContext> {
    let bi = v.get("data")?.get("bookingInfo")?;
    let text = |k: &str| match bi.get(k)? {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    let booking_id = text("bookingId")?;
    let nights = text("durationDays").and_then(|d| d.parse::<i32>().ok());
    Some(PrincessBookingContext {
        booking_id,
        ship_code: text("shipCode")?,
        voyage_code: text("voyageId")?,
        pax_id: text("paxId")?,
        nights,
        sail_date: parse_departure(&text("departureDate")?)?,
        currency: text("bookingCurrency")?,
    })
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
        let body = check_ok(resp).await?;
        let v: serde_json::Value = serde_json::from_str(&body)?;
        Ok(parse_keep_alive(&v))
    }
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
            body: body.chars().take(ERROR_BODY_MAX).collect(),
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
    #[test]
    fn uuid4_shape() {
        let u = super::uuid4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }
}
