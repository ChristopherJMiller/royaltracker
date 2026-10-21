use anyhow::{Context, Result};
use royaltracker_api::princess::PrincessGuestClient;
use royaltracker_api::{
    CruiseClient, PacingConfig, PublicClient, PublicClientConfig, DEFAULT_USER_AGENT,
};
use royaltracker_config::{Config, PacingSettings};
use royaltracker_core::{
    discover_account, run_public_sweep, run_scrape_cycle, sleep_with_jitter, PublicSweepConfig,
};
use royaltracker_crypto::Cipher;
use royaltracker_notify::{Dispatcher, Notifier, WebPushChannel};
use royaltracker_storage::{connect, ActiveAccount, DefaultRepo, PriceRepo};
use royaltracker_telegram::bot;
use royaltracker_types::{Booking, Brand};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

fn pacing(p: &PacingSettings) -> PacingConfig {
    PacingConfig::from_millis(
        p.min_interval_ms,
        p.jitter_lo_ms,
        p.jitter_hi_ms,
        p.max_retries,
        p.base_backoff_ms,
        p.max_backoff_ms,
        p.cooldown_after_challenge_ms,
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();

    let cfg = Config::load().context("loading config")?;

    sleep_with_jitter(cfg.jitter_minutes).await;

    let repo = connect(&cfg.database_url).await.context("db connect")?;
    repo.migrate().await.context("db migrate")?;

    let cipher = Cipher::from_base64(&cfg.encryption_key_b64).context("encryption_key_b64")?;
    let bot = bot(&cfg.telegram.bot_token);
    let mut dispatcher = Dispatcher::new().with_telegram(bot.clone());
    if let Some(wp) = &cfg.notify.web_push {
        match WebPushChannel::new(wp.vapid_private_key_b64.clone(), wp.vapid_subject.clone()) {
            Ok(ch) => {
                dispatcher = dispatcher.with_web_push(ch);
                info!("web-push channel enabled");
            }
            Err(e) => warn!(error = %e, "web push disabled: bad VAPID key"),
        }
    }

    let princess_client = match PrincessGuestClient::new(pacing(&cfg.pacing)) {
        Ok(c) => Some(c),
        Err(e) => {
            warn!(error = %e, "could not build Princess client; Princess prices skipped");
            None
        }
    };

    // Phase P (always runs): the public no-login sweep. Fetches each distinct
    // tracked sailing's public promo prices once and fans out to subscribers.
    match PublicClient::new(PublicClientConfig::new(
        DEFAULT_USER_AGENT.to_string(),
        pacing(&cfg.pacing),
    )) {
        Ok(public_client) => {
            let sweep_cfg = PublicSweepConfig {
                window: Duration::from_secs(cfg.public_sweep_window_minutes * 60),
            };
            let o = run_public_sweep(&public_client, princess_client.as_ref(), &repo as &dyn PriceRepo, &dispatcher, &sweep_cfg).await;
            info!(
                sailings = o.sailings_fetched,
                prices = o.prices_recorded,
                diffs = o.diffs_detected,
                notified = o.diffs_notified,
                failures = o.failures,
                "public sweep done"
            );
        }
        Err(e) => warn!(error = %e, "could not build public client; skipping public sweep"),
    }

    // Phase A: per-account discovery. Each cruise account must log in with its
    // own credentials to enumerate *its* bookings. We collect one authenticated
    // RCG client per brand along the way; whichever account logged in first wins.
    let accounts = repo
        .list_active_cruise_accounts()
        .await
        .context("list cruise accounts")?;
    info!(count = accounts.len(), "cruise accounts to scrape");
    if accounts.is_empty() {
        info!("no active accounts; public-only run complete");
        return Ok(());
    }

    let mut clients_by_brand: HashMap<Brand, CruiseClient> = HashMap::new();
    for active in &accounts {
        match discover_for_account(&repo, &cipher, &dispatcher, &cfg.rcg_basic_auth_b64, active).await {
            Ok(rcg_clients) => {
                for (brand, client) in rcg_clients {
                    clients_by_brand.entry(brand).or_insert(client);
                }
            }
            Err(e) => {
                warn!(account = active.account.id, error = %e, "account discovery failed");
                let _ = royaltracker_telegram::send_text(
                    &bot,
                    active.user.telegram_chat_id,
                    format!("⚠️ Today's price check failed: {e}"),
                )
                .await;
            }
        }
    }

    // Phase B: per-booking price scrape with fan-out. Each watched product is
    // fetched exactly once regardless of how many users share the booking.
    if clients_by_brand.is_empty() && princess_client.is_none() {
        warn!("no authenticated clients available; skipping price scrape");
        return Ok(());
    }

    let watched = repo.list_active_watched().await.context("list watched")?;
    if watched.is_empty() {
        info!("no products configured; nothing to scrape");
        return Ok(());
    }

    let bookings_by_res: HashMap<String, Booking> = repo
        .list_bookings()
        .await
        .context("list bookings")?
        .into_iter()
        .map(|b| (b.reservation_id.clone(), b))
        .collect();

    let outcome = run_scrape_cycle(
        &clients_by_brand,
        princess_client.as_ref(),
        &repo as &dyn PriceRepo,
        &dispatcher,
        &watched,
        &bookings_by_res,
    )
    .await;

    info!(
        snapshots = outcome.snapshots_written,
        diffs = outcome.diffs_detected,
        notified = outcome.diffs_notified,
        watched = watched.len(),
        bookings = bookings_by_res.len(),
        "scrape complete"
    );

    Ok(())
}

async fn discover_for_account(
    repo: &DefaultRepo,
    cipher: &Cipher,
    notifier: &dyn Notifier,
    basic_auth_b64: &str,
    active: &ActiveAccount,
) -> anyhow::Result<HashMap<Brand, CruiseClient>> {
    let password_bytes = cipher.decrypt(&active.account.password_nonce, &active.account.password_ct)?;
    let password = String::from_utf8(password_bytes)?;

    // Pass the notifier so cabin assignments/reassignments detected during
    // discovery are pushed to the reservation's subscribers.
    let d = discover_account(
        repo as &dyn PriceRepo,
        basic_auth_b64,
        &active.user,
        &active.account,
        &password,
        Some(notifier),
    )
    .await?;

    info!(
        account = active.account.id,
        persisted = d.report.persisted,
        logins_ok = d.report.logins_ok,
        brands_authenticated = d.rcg_clients.len(),
        "bookings discovered+persisted"
    );

    Ok(d.rcg_clients)
}
