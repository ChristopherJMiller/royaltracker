//! Send a fake price-drop alert to your admin_chat_id so you can verify the
//! Telegram message format. Reads `bot_token` and `admin_chat_id` from config.toml.
//! Usage: `cargo run -p royaltracker-telegram --example test-alert`

use royaltracker_telegram::{bot, send_diff, DiffContext};
use royaltracker_types::Diff;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let toml: toml::Value =
        toml::from_str(&std::fs::read_to_string("config.toml")?).expect("config.toml");
    let token = toml["telegram"]["bot_token"].as_str().expect("bot_token").to_string();
    // Prefer admin_chat_id; fall back to $ROYALTRACKER_TEST_CHAT_ID for ad-hoc runs.
    let chat_id = toml["telegram"]["admin_chat_id"]
        .as_integer()
        .filter(|v| *v != 0)
        .or_else(|| std::env::var("ROYALTRACKER_TEST_CHAT_ID").ok().and_then(|s| s.parse().ok()))
        .ok_or_else(|| anyhow::anyhow!(
            "set [telegram].admin_chat_id in config.toml, or run with ROYALTRACKER_TEST_CHAT_ID=<id>"
        ))?;

    let b = bot(token);

    let diff = Diff::from_prices(1, 107.99, 87.99);
    let ctx = DiffContext {
        label: "Deluxe Beverage Package (Adult)",
        diff: &diff,
        msrp_label: Some("$135.00"),
    };
    send_diff(&b, chat_id, &ctx).await?;
    println!("sent test alert to chat {chat_id}");
    Ok(())
}
