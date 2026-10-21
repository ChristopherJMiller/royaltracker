use anyhow::{Context, Result};
use royaltracker_api::{
    CruiseClient, CruiseClientConfig, PacingConfig, PublicClient, PublicClientConfig,
    DEFAULT_USER_AGENT,
};
use royaltracker_config::Config;
use royaltracker_crypto::Cipher;
use royaltracker_storage::{connect, DefaultRepo, NewCruiseAccount, NewUser, PriceRepo};
use royaltracker_types::{Brand, CruiseAccount, CruiseLine};
use std::str::FromStr;
use std::sync::Arc;
use teloxide::dispatching::dialogue::{self, InMemStorage};
use teloxide::prelude::*;
use teloxide::types::{
    BotCommand, InlineKeyboardButton, InlineKeyboardMarkup, MenuButton, WebAppInfo,
};
use teloxide::utils::command::BotCommands;
use tracing_subscriber::EnvFilter;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Cruise price-drop bot:")]
enum Command {
    #[command(description = "welcome / introduction")]
    Start,
    #[command(description = "show this help")]
    Help,
    #[command(description = "link a cruise line account (Royal Caribbean & Celebrity, or Princess)")]
    Register,
    #[command(description = "list or remove your linked cruise accounts")]
    Accounts,
    #[command(description = "remove your account (deletes encrypted credentials)")]
    Unregister,
    #[command(description = "test your stored credentials by logging in")]
    Test,
    #[command(description = "show currently-tracked prices")]
    Current,
    #[command(description = "show recent price diffs")]
    History,
    #[command(description = "list watched products")]
    Watch,
    #[command(description = "show pending un-notified diffs")]
    Diff,
    #[command(description = "cancel the current /register flow")]
    Cancel,
}

#[derive(Clone, Default, Debug, serde::Serialize, serde::Deserialize)]
enum DialogueState {
    #[default]
    Idle,
    AwaitingLine,
    AwaitingLogin { line: CruiseLine },
    AwaitingPassword { line: CruiseLine, login: String },
}

/// Cruise lines offered for linking. Part A exposes RCG only; Task 12 adds Princess.
const ENABLED_LINES: &[CruiseLine] = &[CruiseLine::Rcg];

fn line_keyboard(lines: &[CruiseLine]) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(
        lines
            .iter()
            .map(|l| [InlineKeyboardButton::callback(l.label(), format!("line:{l}"))]),
    )
}

type Dialog = Dialogue<DialogueState, InMemStorage<DialogueState>>;

#[derive(Clone)]
struct AppState {
    repo: Arc<DefaultRepo>,
    cipher: Arc<Cipher>,
    rcg_basic_auth_b64: Arc<String>,
    web_app_url: Arc<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();

    let cfg = Config::load().context("loading config")?;
    let repo = Arc::new(connect(&cfg.database_url).await.context("db connect")?);
    repo.migrate().await.context("db migrate")?;

    let cipher = Arc::new(
        Cipher::from_base64(&cfg.encryption_key_b64).context("encryption_key_b64")?,
    );

    let app = AppState {
        repo: repo.clone(),
        cipher: cipher.clone(),
        rcg_basic_auth_b64: Arc::new(cfg.rcg_basic_auth_b64.clone()),
        web_app_url: Arc::new(cfg.web.public_url.clone()),
    };

    let bot = Bot::new(&cfg.telegram.bot_token);

    // Register the chat menu button as a Mini App launcher (always visible at
    // the bottom-left of the chat input).
    if let Ok(url) = url::Url::parse(&cfg.web.public_url) {
        let menu = MenuButton::WebApp {
            text: "Dashboard".into(),
            web_app: WebAppInfo { url },
        };
        if let Err(e) = bot.set_chat_menu_button().menu_button(menu).await {
            tracing::warn!(error = %e, "failed to set chat menu button");
        }
    }
    // Register slash-command list so Telegram auto-completes them.
    let cmds: Vec<BotCommand> = Command::bot_commands().to_vec();
    if let Err(e) = bot.set_my_commands(cmds).await {
        tracing::warn!(error = %e, "failed to set bot commands");
    }

    // axum server (Mini App) — runs alongside the poller in the same tokio runtime.
    let princess = Arc::new(
        royaltracker_api::princess::PrincessGuestClient::new(PacingConfig::from_millis(
            cfg.pacing.min_interval_ms,
            cfg.pacing.jitter_lo_ms,
            cfg.pacing.jitter_hi_ms,
            cfg.pacing.max_retries,
            cfg.pacing.base_backoff_ms,
            cfg.pacing.max_backoff_ms,
            cfg.pacing.cooldown_after_challenge_ms,
        ))
        .context("princess guest client")?,
    );
    let web_state = royaltracker_web::AppState {
        repo: repo.clone(),
        cipher: cipher.clone(),
        bot_token: Arc::new(cfg.telegram.bot_token.clone()),
        rcg_basic_auth_b64: Arc::new(cfg.rcg_basic_auth_b64.clone()),
        princess,
    };
    let web_addr: std::net::SocketAddr = cfg
        .web
        .bind_addr
        .parse()
        .context("web.bind_addr parse")?;

    // Mount the no-login public tier alongside the authed Mini App. The public
    // client egresses from this pod's (home) IP for live seed-on-lookup fetches.
    let p = &cfg.pacing;
    let mut app_router = royaltracker_web::router(web_state);
    match PublicClient::new(PublicClientConfig::new(
        DEFAULT_USER_AGENT.to_string(),
        PacingConfig::from_millis(
            p.min_interval_ms,
            p.jitter_lo_ms,
            p.jitter_hi_ms,
            p.max_retries,
            p.base_backoff_ms,
            p.max_backoff_ms,
            p.cooldown_after_challenge_ms,
        ),
    )) {
        Ok(public_client) => {
            // Identity (subscribe/manage) is optional — read-only lookup works
            // without it. Enabled only when public.enabled + turnstile + device key.
            let identity = cfg.public.as_ref().filter(|p| p.enabled).and_then(|p| {
                match (&p.turnstile_secret, &p.turnstile_site_key, &p.device_cookie_key_b64) {
                    (Some(secret), Some(site), Some(key_b64)) => {
                        let vapid = cfg
                            .notify
                            .web_push
                            .as_ref()
                            .map(|w| w.vapid_public_key_b64.clone());
                        let id = royaltracker_web::PublicIdentity::from_parts(
                            secret.clone(),
                            site.clone(),
                            key_b64,
                            vapid,
                        );
                        if id.is_none() {
                            tracing::warn!("public.device_cookie_key_b64 invalid; subscribe disabled");
                        }
                        id
                    }
                    _ => {
                        tracing::warn!("public.enabled but turnstile/device key missing; subscribe disabled");
                        None
                    }
                }
            });
            let subscribe_on = identity.is_some();
            // Prod hardening: the public router can use a separate least-privilege
            // DB role (walled off from credential tables). Falls back to the main
            // pool in dev / when unset.
            let public_repo = match cfg.public.as_ref().and_then(|p| p.public_database_url.clone()) {
                Some(url) => match connect(&url).await {
                    Ok(r) => {
                        tracing::info!("public tier using least-privilege DB pool");
                        Arc::new(r)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "public_database_url connect failed; using main pool");
                        repo.clone()
                    }
                },
                None => repo.clone(),
            };
            let public_state = royaltracker_web::PublicState {
                repo: public_repo,
                public_client: Arc::new(public_client),
                identity,
            };
            app_router = app_router.merge(royaltracker_web::public_router(public_state));
            tracing::info!(subscribe = subscribe_on, "public no-login tier mounted at /p");
        }
        Err(e) => tracing::warn!(error = %e, "could not build public client; /p disabled"),
    }

    tracing::info!(addr = %web_addr, "starting Mini App HTTP server");
    let listener = tokio::net::TcpListener::bind(web_addr)
        .await
        .context("bind web port")?;
    let web_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app_router).await {
            tracing::error!(error = %e, "web server crashed");
        }
    });

    tracing::info!("royaltracker-bot starting; long-poll mode");
    let handler = dialogue::enter::<Update, InMemStorage<DialogueState>, DialogueState, _>()
        .branch(
            Update::filter_message()
                .filter_command::<Command>()
                .endpoint(on_command),
        )
        .branch(Update::filter_message().endpoint(on_message))
        .branch(Update::filter_callback_query().endpoint(on_callback));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![app, InMemStorage::<DialogueState>::new()])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    web_handle.abort();
    Ok(())
}

async fn on_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    dialog: Dialog,
    app: AppState,
) -> Result<(), teloxide::RequestError> {
    match cmd {
        Command::Start => {
            let greeting = "👋 Welcome to the Cruise Planner price-drop bot.\n\n\
                I watch your cruise add-on prices and ping you when they drop.\n\n\
                Tap the dashboard below to see prices, charts, and manage what you track. \
                If this is your first time, run /register to link a cruise account first.";
            let kb = open_dashboard_keyboard(&app.web_app_url);
            bot.send_message(msg.chat.id, greeting)
                .reply_markup(kb)
                .await?;
        }
        Command::Watch | Command::Current | Command::History | Command::Diff => {
            let kb = open_dashboard_keyboard(&app.web_app_url);
            bot.send_message(
                msg.chat.id,
                "Tap to open the dashboard — all of this lives in the Mini App now.",
            )
            .reply_markup(kb)
            .await?;
        }
        Command::Help => {
            bot.send_message(msg.chat.id, Command::descriptions().to_string())
                .await?;
        }
        Command::Register => {
            dialog.update(DialogueState::AwaitingLine).await.ok();
            bot.send_message(msg.chat.id, "Which cruise line would you like to link?")
                .reply_markup(line_keyboard(ENABLED_LINES))
                .await?;
        }
        Command::Accounts => {
            handle_accounts(&bot, msg.chat.id, &app).await?;
        }
        Command::Cancel => {
            dialog.update(DialogueState::Idle).await.ok();
            bot.send_message(msg.chat.id, "Cancelled.").await?;
        }
        Command::Unregister => {
            if let Err(e) = app.repo.deactivate_user(msg.chat.id.0).await {
                bot.send_message(msg.chat.id, format!("error: {e}")).await?;
            } else {
                bot.send_message(
                    msg.chat.id,
                    "Your account is deactivated. Run /register to set it up again.",
                )
                .await?;
            }
        }
        Command::Test => {
            handle_test(&bot, &msg, &app).await?;
        }
    }
    Ok(())
}

fn open_dashboard_keyboard(url: &str) -> InlineKeyboardMarkup {
    let btn = match url::Url::parse(url) {
        Ok(parsed) => InlineKeyboardButton::web_app(
            "🛳️ Open dashboard",
            WebAppInfo { url: parsed },
        ),
        Err(_) => InlineKeyboardButton::callback("Dashboard URL not configured", "noop"),
    };
    InlineKeyboardMarkup::new([[btn]])
}

async fn on_message(
    bot: Bot,
    msg: Message,
    dialog: Dialog,
    app: AppState,
) -> Result<(), teloxide::RequestError> {
    let state = dialog.get().await.ok().flatten().unwrap_or_default();
    let Some(text) = msg.text().map(str::trim).map(str::to_owned) else {
        return Ok(());
    };

    match state {
        DialogueState::Idle => {
            // ignore stray messages
        }
        DialogueState::AwaitingLine => {
            bot.send_message(
                msg.chat.id,
                "Tap one of the buttons above, or /cancel to start over.",
            )
            .await?;
        }
        DialogueState::AwaitingLogin { line } => {
            if line == CruiseLine::Rcg && !text.contains('@') {
                bot.send_message(msg.chat.id, "That doesn't look like an email. Try again, or /cancel.")
                    .await?;
                return Ok(());
            }
            dialog
                .update(DialogueState::AwaitingPassword { line, login: text })
                .await
                .ok();
            bot.send_message(
                msg.chat.id,
                "Got it. Now send your password.\n\n\
                 ⚠️ I'll delete your password message immediately after I read it. \
                 It will be stored encrypted at rest.",
            )
            .await?;
        }
        DialogueState::AwaitingPassword { line, login } => {
            // Delete the password message ASAP, regardless of whether storage succeeds.
            let _ = bot.delete_message(msg.chat.id, msg.id).await;
            dialog.update(DialogueState::Idle).await.ok();
            let tg_username = msg.from.as_ref().and_then(|u| u.username.clone());
            link_account(&bot, msg.chat.id, tg_username, line, login, text, &app).await?;
        }
    }
    Ok(())
}

async fn link_account(
    bot: &Bot,
    chat: ChatId,
    tg_username: Option<String>,
    line: CruiseLine,
    login: String,
    password: String,
    app: &AppState,
) -> Result<(), teloxide::RequestError> {
    let (nonce, ct) = match app.cipher.encrypt(password.as_bytes()) {
        Ok(p) => p,
        Err(e) => {
            bot.send_message(chat, format!("encryption error: {e}")).await?;
            return Ok(());
        }
    };
    let user_id = match app
        .repo
        .upsert_user(&NewUser {
            telegram_chat_id: chat.0,
            telegram_username: tg_username.as_deref(),
        })
        .await
    {
        Ok(id) => id,
        Err(e) => {
            bot.send_message(chat, format!("storage error: {e}")).await?;
            return Ok(());
        }
    };
    let account_id = match app
        .repo
        .upsert_cruise_account(&NewCruiseAccount {
            user_id,
            line,
            login: &login,
            password_ct: &ct,
            password_nonce: &nonce,
        })
        .await
    {
        Ok(id) => id,
        Err(e) => {
            bot.send_message(chat, format!("storage error: {e}")).await?;
            return Ok(());
        }
    };

    bot.send_message(chat, format!("✅ Stored. Testing {} login…", line.label()))
        .await?;

    let outcome = test_login(app, line, &login, &password).await;
    let err_text = outcome.as_ref().err().map(|e| e.to_string());
    app.repo
        .record_login_outcome(account_id, err_text.as_deref())
        .await
        .ok();
    match outcome {
        Ok(_) => {
            bot.send_message(
                chat,
                format!(
                    "✅ {} login succeeded.\n\n\
                     Use /accounts to manage linked accounts, /unregister to remove everything.",
                    line.label()
                ),
            )
            .await
            .ok();
        }
        Err(e) => {
            bot.send_message(
                chat,
                format!(
                    "⚠️ Stored your credentials, but the login test failed:\n{e}\n\n\
                     If this was a typo, /register again to overwrite. \
                     Otherwise wait 15+ minutes (rate-limit) and /test."
                ),
            )
            .await
            .ok();
        }
    }
    Ok(())
}

async fn on_callback(
    bot: Bot,
    q: CallbackQuery,
    dialog: Dialog,
    app: AppState,
) -> Result<(), teloxide::RequestError> {
    let Some(data) = q.data.clone() else {
        return Ok(());
    };
    bot.answer_callback_query(q.id.clone()).await?;

    let Some(chat) = q.message.as_ref().map(|m| m.chat().id) else {
        return Ok(());
    };

    if let Some(line_str) = data.strip_prefix("line:") {
        let Ok(line) = CruiseLine::from_str(line_str) else {
            return Ok(());
        };
        if !ENABLED_LINES.contains(&line) {
            return Ok(());
        }
        let state = dialog.get().await.ok().flatten().unwrap_or_default();
        if !matches!(state, DialogueState::AwaitingLine) {
            bot.send_message(chat, "No registration in progress. Use /register to start.")
                .await?;
            return Ok(());
        }
        dialog.update(DialogueState::AwaitingLogin { line }).await.ok();
        let prompt = match line {
            CruiseLine::Rcg => "What's your account email address?",
            CruiseLine::Princess => "What's your Princess account login?",
        };
        bot.send_message(chat, format!("Linking {}. {prompt}", line.label()))
            .await?;
    } else if let Some(line_str) = data.strip_prefix("rmacct:") {
        let Ok(line) = CruiseLine::from_str(line_str) else {
            return Ok(());
        };
        let msg = match app.repo.get_user_by_chat_id(chat.0).await {
            Ok(Some(user)) => match app.repo.remove_cruise_account(user.id, line).await {
                Ok(true) => format!("Removed your {} account.", line.label()),
                Ok(false) => format!("No {} account was linked.", line.label()),
                Err(e) => format!("error: {e}"),
            },
            Ok(None) => "You have no linked accounts.".to_string(),
            Err(e) => format!("error: {e}"),
        };
        bot.send_message(chat, msg).await?;
    }
    Ok(())
}

async fn user_accounts(app: &AppState, chat: ChatId) -> Vec<CruiseAccount> {
    match app.repo.get_user_by_chat_id(chat.0).await {
        Ok(Some(user)) => app.repo.list_cruise_accounts(user.id).await.unwrap_or_default(),
        _ => Vec::new(),
    }
}

async fn handle_accounts(
    bot: &Bot,
    chat: ChatId,
    app: &AppState,
) -> Result<(), teloxide::RequestError> {
    let accounts = user_accounts(app, chat).await;
    if accounts.is_empty() {
        bot.send_message(chat, "No linked accounts. Use /register to link one.")
            .await?;
        return Ok(());
    }
    let mut text = String::from("Your linked accounts:\n");
    let mut rows = Vec::new();
    for a in &accounts {
        let status = match &a.last_login_error {
            Some(e) => format!("⚠️ {e}"),
            None => "✅".to_string(),
        };
        text.push_str(&format!("• {} · {} · {status}\n", a.line.label(), a.login));
        rows.push([InlineKeyboardButton::callback(
            format!("Remove {}", a.line.label()),
            format!("rmacct:{}", a.line),
        )]);
    }
    bot.send_message(chat, text)
        .reply_markup(InlineKeyboardMarkup::new(rows))
        .await?;
    Ok(())
}

async fn handle_test(bot: &Bot, msg: &Message, app: &AppState) -> Result<(), teloxide::RequestError> {
    let chat_id = msg.chat.id;
    let accounts = user_accounts(app, chat_id).await;
    if accounts.is_empty() {
        bot.send_message(chat_id, "Not registered — use /register first.").await?;
        return Ok(());
    }
    for a in &accounts {
        let outcome = test_account(app, a).await;
        let err_text = outcome.as_ref().err().map(|e| e.to_string());
        app.repo.record_login_outcome(a.id, err_text.as_deref()).await.ok();
        match outcome {
            Ok(_) => {
                bot.send_message(chat_id, format!("✅ {} login OK.", a.line.label())).await?;
            }
            Err(e) => {
                bot.send_message(chat_id, format!("❌ {} login failed: {e}", a.line.label()))
                    .await?;
            }
        }
    }
    Ok(())
}

async fn test_account(app: &AppState, a: &CruiseAccount) -> anyhow::Result<String> {
    let pw_bytes = app.cipher.decrypt(&a.password_nonce, &a.password_ct)?;
    let password = String::from_utf8(pw_bytes)?;
    test_login(app, a.line, &a.login, &password).await
}

/// Logs in for the given line; returns an account identifier on success.
async fn test_login(
    app: &AppState,
    line: CruiseLine,
    login: &str,
    password: &str,
) -> anyhow::Result<String> {
    match line {
        CruiseLine::Rcg => {
            let api_cfg = CruiseClientConfig::web(
                Brand::Royal,
                login.to_string(),
                password.to_string(),
                app.rcg_basic_auth_b64.as_ref().clone(),
            );
            let api = CruiseClient::new(api_cfg)?;
            let token = api.login().await?;
            Ok(token.account_id)
        }
        CruiseLine::Princess => anyhow::bail!("Princess login is not enabled yet"),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_keyboard_uses_enabled_lines() {
        let kb = line_keyboard(&[CruiseLine::Rcg]);
        let json = serde_json::to_string(&kb).unwrap();
        assert!(json.contains("line:rcg"));
        assert!(!json.contains("line:princess"));
        assert!(json.contains("Royal Caribbean & Celebrity"));
    }
}
