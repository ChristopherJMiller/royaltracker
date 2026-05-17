use anyhow::{Context, Result};
use royaltracker_api::CruiseClient;
use royaltracker_config::Config;
use royaltracker_core::{discover_with_clients, run_scrape_cycle, sleep_with_jitter};
use royaltracker_crypto::Cipher;
use royaltracker_storage::{connect, DefaultRepo, PriceRepo};
use royaltracker_telegram::{bot, Bot};
use royaltracker_types::{Booking, Brand, User};
use std::collections::HashMap;
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

    // Phase A: per-user discovery. Each user must log in with their own
    // credentials to enumerate *their* bookings. We collect one authenticated
    // client per brand along the way; whichever user logged in first wins.
    let users = repo.list_active_users().await.context("list users")?;
    info!(count = users.len(), "users to scrape");

    let mut clients_by_brand: HashMap<Brand, CruiseClient> = HashMap::new();
    for user in &users {
        let label = user.rcg_username.clone();
        match discover_for_user(&repo, &cipher, &bot, &cfg.rcg_basic_auth_b64, user).await {
            Ok(user_clients) => {
                for (brand, client) in user_clients {
                    clients_by_brand.entry(brand).or_insert(client);
                }
            }
            Err(e) => {
                warn!(user = %label, error = %e, "user discovery failed");
                let _ = royaltracker_telegram::send_text(
                    &bot,
                    user.telegram_chat_id,
                    format!("⚠️ Today's price check failed: {e}"),
                )
                .await;
            }
        }
    }

    // Phase B: per-booking price scrape with fan-out. Each watched product is
    // fetched exactly once regardless of how many users share the booking.
    if clients_by_brand.is_empty() {
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
        &repo as &dyn PriceRepo,
        &bot,
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

async fn discover_for_user(
    repo: &DefaultRepo,
    cipher: &Cipher,
    _bot: &Bot,
    basic_auth_b64: &str,
    user: &User,
) -> anyhow::Result<HashMap<Brand, CruiseClient>> {
    let password_bytes = cipher.decrypt(&user.rcg_password_nonce, &user.rcg_password_ct)?;
    let password = String::from_utf8(password_bytes)?;

    let (report, clients) = discover_with_clients(
        &user.rcg_username,
        &password,
        basic_auth_b64,
        repo as &dyn PriceRepo,
        user,
    )
    .await?;

    info!(
        user = %user.rcg_username,
        persisted = report.persisted,
        logins_ok = report.logins_ok,
        brands_authenticated = clients.len(),
        "bookings discovered+persisted"
    );

    Ok(clients)
}
