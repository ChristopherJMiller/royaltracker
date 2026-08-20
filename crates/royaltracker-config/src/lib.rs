use figment::providers::{Env, Format, Toml};
use figment::Figment;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub database_url: String,
    pub telegram: TelegramConfig,
    pub web: WebConfig,
    /// Shared RCG OAuth basic-auth (the public client_id:secret from the JS bundle).
    /// Same for all users — they only supply their own username + password via /register.
    pub rcg_basic_auth_b64: String,
    /// 32-byte ChaCha20-Poly1305 key, base64-encoded.
    /// Generate once via `royaltracker_crypto::Cipher::generate_key_b64()` and persist out-of-band.
    pub encryption_key_b64: String,
    #[serde(default = "default_jitter_minutes")]
    pub jitter_minutes: u32,

    // --- public tier (all defaulted/optional so authed-only deploys still boot) ---
    /// Pacing/backoff for the public price sweep.
    #[serde(default)]
    pub pacing: PacingSettings,
    /// Wall-clock window (minutes) to drip the public sweep across.
    #[serde(default = "default_public_sweep_window")]
    pub public_sweep_window_minutes: u64,
    /// Notification channels for the public tier (web push / email).
    #[serde(default)]
    pub notify: NotifyConfig,
    /// Public web tier. When absent, only the authed Mini App is served.
    #[serde(default)]
    pub public: Option<PublicConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PacingSettings {
    pub min_interval_ms: u64,
    pub jitter_lo_ms: u64,
    pub jitter_hi_ms: u64,
    pub max_retries: u32,
    pub base_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub cooldown_after_challenge_ms: u64,
}

impl Default for PacingSettings {
    fn default() -> Self {
        Self {
            min_interval_ms: 3000,
            jitter_lo_ms: 2000,
            jitter_hi_ms: 8000,
            max_retries: 4,
            base_backoff_ms: 30_000,
            max_backoff_ms: 900_000,
            cooldown_after_challenge_ms: 600_000,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NotifyConfig {
    #[serde(default)]
    pub web_push: Option<WebPushConfig>,
    #[serde(default)]
    pub email: Option<EmailConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebPushConfig {
    pub vapid_private_key_b64: String,
    /// The one source of truth for the VAPID public key; the web tier serves it.
    pub vapid_public_key_b64: String,
    /// VAPID `sub` claim, e.g. `mailto:you@example.com`.
    pub vapid_subject: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmailConfig {
    /// SMTP connection URL, e.g. `smtps://user:pass@host:465`.
    pub smtp_url: String,
    pub from_address: String,
    /// Base URL for verify links, e.g. `https://prices.example.com`.
    pub verify_base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PublicConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Least-privilege DB URL for the public web service (prod). Falls back to
    /// the main `database_url` when absent (e.g. dev SQLite, single-tenant).
    #[serde(default)]
    pub public_database_url: Option<String>,
    /// 32-byte HMAC key (base64) for signing the device cookie. Required to enable
    /// subscribe/manage routes; read-only lookup needs none.
    #[serde(default)]
    pub device_cookie_key_b64: Option<String>,
    #[serde(default)]
    pub turnstile_site_key: Option<String>,
    #[serde(default)]
    pub turnstile_secret: Option<String>,
    /// Public hostname fronted by the Cloudflare Tunnel (for CORS / links).
    #[serde(default)]
    pub tunnel_hostname: Option<String>,
    /// Optional separate bind address for a standalone public listener.
    #[serde(default)]
    pub public_bind_addr: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebConfig {
    /// Public HTTPS URL the Mini App is served from — used in the inline `web_app` button.
    /// Dev: a Cloudflare quick-tunnel URL (`cloudflared tunnel --url http://localhost:8080`).
    /// Prod: e.g. `https://rccl-tracker.chrismiller.xyz`.
    pub public_url: String,
    /// Local bind address for the axum HTTP server. Defaults to 0.0.0.0:8080.
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
}

fn default_bind_addr() -> String {
    "0.0.0.0:8080".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    /// Optional admin chat for system-level messages (startup, errors).
    /// Per-user diffs go to that user's own chat_id stored in the users table.
    #[serde(default)]
    pub admin_chat_id: Option<i64>,
}

fn default_jitter_minutes() -> u32 {
    10
}

fn default_public_sweep_window() -> u64 {
    150
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("figment: {0}")]
    Figment(#[from] figment::Error),
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let figment = Figment::new()
            .merge(Toml::file("config.toml"))
            .merge(Env::prefixed("ROYALTRACKER_").split("__"));
        Ok(figment.extract()?)
    }
}
