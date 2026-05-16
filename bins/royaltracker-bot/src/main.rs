use anyhow::{Context, Result};
use royaltracker_api::{CruiseClient, CruiseClientConfig};
use royaltracker_config::Config;
use royaltracker_crypto::Cipher;
use royaltracker_storage::{connect, DefaultRepo, NewUser, PriceRepo};
use royaltracker_types::Brand;
use std::str::FromStr;
use std::sync::Arc;
use teloxide::dispatching::dialogue::{self, InMemStorage};
use teloxide::prelude::*;
use teloxide::types::{
    BotCommand, InlineKeyboardButton, InlineKeyboardMarkup, MenuButton, ParseMode, WebAppInfo,
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
    #[command(description = "register your Royal Caribbean / Celebrity account")]
    Register,
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
    AwaitingEmail,
    AwaitingPassword { email: String },
    AwaitingBrand { email: String, password: String },
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
    let web_state = royaltracker_web::AppState {
        repo: repo.clone(),
        cipher: cipher.clone(),
        bot_token: Arc::new(cfg.telegram.bot_token.clone()),
        rcg_basic_auth_b64: Arc::new(cfg.rcg_basic_auth_b64.clone()),
    };
    let web_addr: std::net::SocketAddr = cfg
        .web
        .bind_addr
        .parse()
        .context("web.bind_addr parse")?;
    tracing::info!(addr = %web_addr, "starting Mini App HTTP server");
    let listener = tokio::net::TcpListener::bind(web_addr)
        .await
        .context("bind web port")?;
    let web_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, royaltracker_web::router(web_state)).await {
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
                I watch your Royal Caribbean / Celebrity add-on prices and ping you when they drop.\n\n\
                Tap the dashboard below to see prices, charts, and manage what you track. \
                If this is your first time, run /register to link your RCG account first.";
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
            dialog.update(DialogueState::AwaitingEmail).await.ok();
            bot.send_message(
                msg.chat.id,
                "Let's set up your Royal Caribbean / Celebrity account.\n\n\
                 What's your account email address?",
            )
            .await?;
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
    _app: AppState,
) -> Result<(), teloxide::RequestError> {
    let state = dialog.get().await.ok().flatten().unwrap_or_default();
    let Some(text) = msg.text().map(str::trim).map(str::to_owned) else {
        return Ok(());
    };

    match state {
        DialogueState::Idle => {
            // ignore stray messages
        }
        DialogueState::AwaitingEmail => {
            if !text.contains('@') {
                bot.send_message(msg.chat.id, "That doesn't look like an email. Try again, or /cancel.")
                    .await?;
                return Ok(());
            }
            dialog
                .update(DialogueState::AwaitingPassword { email: text })
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
        DialogueState::AwaitingPassword { email } => {
            // Delete the password message ASAP, regardless of whether storage succeeds.
            let _ = bot.delete_message(msg.chat.id, msg.id).await;

            // Stash the password in dialogue state through the brand-pick step.
            dialog
                .update(DialogueState::AwaitingBrand {
                    email,
                    password: text,
                })
                .await
                .ok();

            let kb = InlineKeyboardMarkup::new([[
                InlineKeyboardButton::callback("Royal Caribbean", "brand:royal"),
                InlineKeyboardButton::callback("Celebrity", "brand:celebrity"),
            ]]);
            bot.send_message(
                msg.chat.id,
                "Password received and your message has been deleted.\n\n\
                 Which cruise line is your account under?",
            )
            .reply_markup(kb)
            .await?;
        }
        DialogueState::AwaitingBrand { .. } => {
            bot.send_message(
                msg.chat.id,
                "Tap one of the buttons above, or /cancel to start over.",
            )
            .await?;
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

    if let Some(brand_str) = data.strip_prefix("brand:") {
        let brand = match Brand::from_str(brand_str) {
            Ok(b) => b,
            Err(_) => return Ok(()),
        };

        let state = dialog.get().await.ok().flatten().unwrap_or_default();
        let DialogueState::AwaitingBrand { email, password } = state else {
            bot.send_message(chat, "No registration in progress. Use /register to start.")
                .await?;
            return Ok(());
        };

        let (nonce, ct) = match app.cipher.encrypt(password.as_bytes()) {
            Ok(p) => p,
            Err(e) => {
                bot.send_message(chat, format!("encryption error: {e}"))
                    .await?;
                return Ok(());
            }
        };

        let tg_user = q.from.username.clone();
        let new_user = NewUser {
            telegram_chat_id: chat.0,
            telegram_username: tg_user.as_deref(),
            rcg_username: &email,
            rcg_password_ct: &ct,
            rcg_password_nonce: &nonce,
            brand_pref: brand,
        };

        if let Err(e) = app.repo.upsert_user(&new_user).await {
            bot.send_message(chat, format!("storage error: {e}")).await?;
            dialog.update(DialogueState::Idle).await.ok();
            return Ok(());
        }

        dialog.update(DialogueState::Idle).await.ok();
        bot.send_message(
            chat,
            format!(
                "✅ Stored. Testing login against {}...",
                match brand {
                    Brand::Royal => "Royal Caribbean",
                    Brand::Celebrity => "Celebrity",
                }
            ),
        )
        .await?;

        match try_login(&app, chat.0).await {
            Ok(account_id) => {
                bot.send_message(
                    chat,
                    format!(
                        "✅ Login succeeded.\nYour RCG accountId: `{account_id}`\n\n\
                         Use /watch to manage tracked products, /unregister to remove your account."
                    ),
                )
                .parse_mode(ParseMode::MarkdownV2)
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
    }
    Ok(())
}

async fn handle_test(bot: &Bot, msg: &Message, app: &AppState) -> Result<(), teloxide::RequestError> {
    let chat_id = msg.chat.id;
    match try_login(app, chat_id.0).await {
        Ok(account_id) => {
            bot.send_message(chat_id, format!("✅ Login OK. accountId={account_id}"))
                .await?;
        }
        Err(e) => {
            bot.send_message(chat_id, format!("❌ Login failed: {e}")).await?;
        }
    }
    Ok(())
}

async fn try_login(app: &AppState, chat_id: i64) -> anyhow::Result<String> {
    let user = app
        .repo
        .get_user_by_chat_id(chat_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not registered — use /register first"))?;

    let pw_bytes = app
        .cipher
        .decrypt(&user.rcg_password_nonce, &user.rcg_password_ct)?;
    let password = String::from_utf8(pw_bytes)?;

    let api_cfg = CruiseClientConfig::web(
        user.brand_pref,
        user.rcg_username.clone(),
        password,
        app.rcg_basic_auth_b64.as_ref().clone(),
    );
    let api = CruiseClient::new(api_cfg)?;
    let token = api.login().await?;
    Ok(token.account_id)
}

