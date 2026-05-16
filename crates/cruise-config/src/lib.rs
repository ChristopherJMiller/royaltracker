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
    /// Generate once via `cruise_crypto::Cipher::generate_key_b64()` and persist out-of-band.
    pub encryption_key_b64: String,
    #[serde(default = "default_jitter_minutes")]
    pub jitter_minutes: u32,
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

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("figment: {0}")]
    Figment(#[from] figment::Error),
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let figment = Figment::new()
            .merge(Toml::file("config.toml"))
            .merge(Env::prefixed("CRUISE_").split("__"));
        Ok(figment.extract()?)
    }
}
