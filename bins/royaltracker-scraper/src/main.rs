use anyhow::{Context, Result};
use royaltracker_api::{BookingSummary, CruiseClient, CruiseClientConfig};
use royaltracker_config::Config;
use royaltracker_core::{
    discover_and_persist_bookings, run_scrape_cycle, sleep_with_jitter, ScrapeContext,
};
use royaltracker_crypto::Cipher;
use royaltracker_storage::{connect, DefaultRepo, PriceRepo};
use royaltracker_telegram::bot;
use royaltracker_types::User;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

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

    let users = repo.list_active_users().await.context("list users")?;
    info!(count = users.len(), "users to scrape");

    for user in users {
        let label = user.rcg_username.clone();
        if let Err(e) =
            scrape_one_user(&repo, &cipher, &bot, &cfg.rcg_basic_auth_b64, &user).await
        {
            warn!(user = %label, error = %e, "user scrape failed");
            // Best-effort notify the user that their run failed.
            let _ = royaltracker_telegram::send_text(
                &bot,
                user.telegram_chat_id,
                format!("⚠️ Today's price check failed: {e}"),
            )
            .await;
        }
    }

    Ok(())
}

async fn scrape_one_user(
    repo: &DefaultRepo,
    cipher: &Cipher,
    bot: &royaltracker_telegram::Bot,
    basic_auth_b64: &str,
    user: &User,
) -> anyhow::Result<()> {
    let password_bytes = cipher.decrypt(&user.rcg_password_nonce, &user.rcg_password_ct)?;
    let password = String::from_utf8(password_bytes)?;

    // Discover bookings across both brands (two logins, both JWTs cached).
    let report = discover_and_persist_bookings(
        &user.rcg_username,
        &password,
        basic_auth_b64,
        repo as &dyn PriceRepo,
        user,
    )
    .await?;
    let api_cfg = CruiseClientConfig::web(
        user.brand_pref,
        user.rcg_username.clone(),
        password,
        basic_auth_b64.to_string(),
    );
    info!(
        user = %user.rcg_username,
        persisted = report.persisted,
        logins_ok = report.logins_ok,
        "bookings discovered+persisted"
    );

    // For the price-fetch loop we still need a client to call catalog/v2 on.
    // Use the user's brand_pref auth host (legacy default = Royal). If we
    // later need cross-brand catalog lookups, we'll build the right client
    // per booking.
    let api = CruiseClient::new(api_cfg)?;
    let _token = api.login().await?;

    let bookings: Vec<BookingSummary> = api
        .list_all_bookings()
        .await?
        .into_iter()
        .map(|(_, s)| s)
        .collect();

    let watched = repo.list_active_watched().await?;
    if watched.is_empty() {
        info!(user = %user.rcg_username, "no products configured; skipping price loop");
        return Ok(());
    }

    let booking_index = build_booking_index(&bookings);

    let outcome = run_scrape_cycle(
        &api,
        repo as &dyn PriceRepo,
        bot,
        user.telegram_chat_id,
        &watched,
        |w| booking_index.get(w.reservation_id.as_str()).cloned(),
    )
    .await;

    info!(
        user = %user.rcg_username,
        snapshots = outcome.snapshots_written,
        diffs = outcome.diffs_detected,
        notified = outcome.diffs_notified,
        "user scrape complete"
    );

    Ok(())
}

fn build_booking_index(
    bookings: &[BookingSummary],
) -> std::collections::HashMap<String, ScrapeContext> {
    bookings
        .iter()
        .filter_map(|b| {
            let res = b.reservation_id.as_ref()?.clone();
            let ship = b.ship_code.clone().unwrap_or_default();
            let pax = b.primary_passenger_id.clone().unwrap_or_default();
            // RCG wire format is YYYYMMDD (no dashes); fall back to YYYY-MM-DD just in case.
            let sail = b.sail_date.as_ref().and_then(|s| {
                chrono::NaiveDate::parse_from_str(s, "%Y%m%d")
                    .or_else(|_| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d"))
                    .ok()
            }).unwrap_or_else(|| chrono::Utc::now().date_naive());
            Some((
                res,
                ScrapeContext {
                    ship_code: ship,
                    passenger_id: pax,
                    start_date: sail,
                },
            ))
        })
        .collect()
}
