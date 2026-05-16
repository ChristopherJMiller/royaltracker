use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Brand {
    Royal,
    Celebrity,
}

impl Brand {
    pub fn host(self) -> &'static str {
        match self {
            Brand::Royal => "www.royalcaribbean.com",
            Brand::Celebrity => "www.celebritycruises.com",
        }
    }

    pub fn api_host(self) -> &'static str {
        "aws-prd.api.rccl.com"
    }

    pub fn code(self) -> &'static str {
        match self {
            Brand::Royal => "R",
            Brand::Celebrity => "C",
        }
    }

    pub fn url_segment(self) -> &'static str {
        match self {
            Brand::Royal => "royal",
            Brand::Celebrity => "celebrity",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Brand::Royal => "royal",
            Brand::Celebrity => "celebrity",
        }
    }
}

impl std::fmt::Display for Brand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Brand {
    type Err = ParseBrandError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "royal" | "r" | "rcl" => Ok(Brand::Royal),
            "celebrity" | "c" | "x" => Ok(Brand::Celebrity),
            _ => Err(ParseBrandError(s.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown brand: {0}")]
pub struct ParseBrandError(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub telegram_chat_id: i64,
    pub telegram_username: Option<String>,
    pub rcg_username: String,
    pub rcg_password_ct: Vec<u8>,
    pub rcg_password_nonce: Vec<u8>,
    pub brand_pref: Brand,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Booking {
    pub reservation_id: String,
    pub brand: Brand,
    pub account_id: String,
    pub ship_code: String,
    pub sail_date: NaiveDate,
    pub passenger_id: Option<String>,
    pub user_id: Option<i64>,
    pub nights: Option<i32>,
    pub package_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchedProduct {
    pub id: i64,
    pub reservation_id: String,
    pub category_prefix: String,
    pub product_code: String,
    pub label: Option<String>,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceSnapshot {
    pub watched_id: i64,
    pub fetched_at: DateTime<Utc>,
    pub adult_promo_price: Option<f64>,
    pub child_promo_price: Option<f64>,
    pub raw_response: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diff {
    pub watched_id: i64,
    pub detected_at: DateTime<Utc>,
    pub old_price: f64,
    pub new_price: f64,
    pub delta_pct: f64,
    pub notified: bool,
}

impl Diff {
    pub fn from_prices(watched_id: i64, old: f64, new: f64) -> Self {
        let delta_pct = if old > 0.0 {
            ((new - old) / old) * 100.0
        } else {
            0.0
        };
        Self {
            watched_id,
            detected_at: Utc::now(),
            old_price: old,
            new_price: new,
            delta_pct,
            notified: false,
        }
    }

    pub fn is_drop(&self) -> bool {
        self.new_price < self.old_price
    }
}
