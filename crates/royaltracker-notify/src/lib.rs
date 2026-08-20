//! Channel-agnostic notification dispatch. Decouples price-drop / text alerts
//! from teloxide so the public tier can deliver via Web Push or email while the
//! authed tier keeps its byte-identical Telegram output.

use async_trait::async_trait;
use royaltracker_types::Diff;
use royaltracker_telegram::{send_diff, send_text, Bot, DiffContext};

/// Where a single alert is delivered. Core maps a stored subscriber
/// (Telegram chat, web-push registration, or email) to one of these.
#[derive(Debug, Clone)]
pub enum NotifyTarget {
    Telegram { chat_id: i64 },
    WebPush(Box<PushSubscription>),
    Email { address: String },
}

#[derive(Debug, Clone)]
pub struct PushSubscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

/// A price-drop alert, transport-agnostic. Channels render it their own way;
/// the Telegram channel reproduces the legacy message byte-for-byte.
pub struct PriceDropAlert<'a> {
    pub label: &'a str,
    pub diff: &'a Diff,
    pub msrp_label: Option<&'a str>,
    pub itinerary: Option<&'a str>,
    pub manage_url: Option<&'a str>,
}

/// A pre-rendered text alert (e.g. cabin assignment). `body` is emitted verbatim.
pub struct TextAlert<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub manage_url: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    /// The endpoint is permanently dead (web-push 404/410) — caller should
    /// deactivate the subscription.
    #[error("subscription gone")]
    SubscriptionGone,
    #[error("transient: {0}")]
    Transient(String),
    #[error("permanent: {0}")]
    Permanent(String),
}

#[async_trait]
pub trait Notifier: Send + Sync {
    async fn notify_price_drop(
        &self,
        target: &NotifyTarget,
        alert: &PriceDropAlert<'_>,
    ) -> Result<(), NotifyError>;
    async fn notify_text(
        &self,
        target: &NotifyTarget,
        alert: &TextAlert<'_>,
    ) -> Result<(), NotifyError>;
}

/// The Telegram channel — a thin wrapper over `royaltracker-telegram` so output
/// is unchanged from before the abstraction existed.
pub struct TelegramChannel {
    bot: Bot,
}

impl TelegramChannel {
    pub fn new(bot: Bot) -> Self {
        Self { bot }
    }

    async fn send_price_drop(
        &self,
        chat_id: i64,
        alert: &PriceDropAlert<'_>,
    ) -> Result<(), NotifyError> {
        let ctx = DiffContext {
            label: alert.label,
            diff: alert.diff,
            msrp_label: alert.msrp_label,
            itinerary: alert.itinerary,
        };
        send_diff(&self.bot, chat_id, &ctx)
            .await
            .map_err(|e| NotifyError::Transient(e.to_string()))
    }

    async fn send_text(&self, chat_id: i64, alert: &TextAlert<'_>) -> Result<(), NotifyError> {
        send_text(&self.bot, chat_id, alert.body.to_string())
            .await
            .map_err(|e| NotifyError::Transient(e.to_string()))
    }
}

/// Routes an alert to the channel matching its `NotifyTarget` variant. Channels
/// that aren't configured/compiled return `Permanent` so the caller can log and
/// move on without failing the whole sweep.
#[derive(Default)]
pub struct Dispatcher {
    telegram: Option<TelegramChannel>,
    #[cfg(feature = "webpush")]
    web_push: Option<webpush::WebPushChannel>,
    #[cfg(feature = "email")]
    email: Option<email::EmailChannel>,
}

impl Dispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_telegram(mut self, bot: Bot) -> Self {
        self.telegram = Some(TelegramChannel::new(bot));
        self
    }
}

#[async_trait]
impl Notifier for Dispatcher {
    async fn notify_price_drop(
        &self,
        target: &NotifyTarget,
        alert: &PriceDropAlert<'_>,
    ) -> Result<(), NotifyError> {
        match target {
            NotifyTarget::Telegram { chat_id } => match &self.telegram {
                Some(c) => c.send_price_drop(*chat_id, alert).await,
                None => Err(NotifyError::Permanent("telegram not configured".into())),
            },
            NotifyTarget::WebPush(_sub) => {
                #[cfg(feature = "webpush")]
                if let Some(c) = &self.web_push {
                    return c.send_price_drop(_sub, alert).await;
                }
                Err(NotifyError::Permanent("web push not configured".into()))
            }
            NotifyTarget::Email { address: _address } => {
                #[cfg(feature = "email")]
                if let Some(c) = &self.email {
                    return c.send_price_drop(_address, alert).await;
                }
                Err(NotifyError::Permanent("email not configured".into()))
            }
        }
    }

    async fn notify_text(
        &self,
        target: &NotifyTarget,
        alert: &TextAlert<'_>,
    ) -> Result<(), NotifyError> {
        match target {
            NotifyTarget::Telegram { chat_id } => match &self.telegram {
                Some(c) => c.send_text(*chat_id, alert).await,
                None => Err(NotifyError::Permanent("telegram not configured".into())),
            },
            // Text alerts (cabin assignments) are an authed-tier concern; public
            // channels only receive price drops for now.
            NotifyTarget::WebPush(_) | NotifyTarget::Email { .. } => {
                Err(NotifyError::Permanent("text alert unsupported on this channel".into()))
            }
        }
    }
}

#[cfg(feature = "webpush")]
mod webpush;
#[cfg(feature = "email")]
mod email;
