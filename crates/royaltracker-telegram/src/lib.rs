use royaltracker_types::Diff;
use teloxide::prelude::*;
use teloxide::types::{ChatId, ParseMode};

pub use teloxide::Bot;

#[derive(Debug, thiserror::Error)]
pub enum TelegramError {
    #[error("telegram: {0}")]
    Request(#[from] teloxide::RequestError),
}

pub fn bot(token: impl Into<String>) -> Bot {
    Bot::new(token)
}

/// Context surfaced alongside a Diff so the alert message can be richer than
/// just "old → new". Optional fields just get omitted if missing.
pub struct DiffContext<'a> {
    pub label: &'a str,
    pub diff: &'a Diff,
    /// Formatted MSRP/base price string from catalog_cache, e.g. "$135.00".
    /// Used to render "X% off MSRP $Y".
    pub msrp_label: Option<&'a str>,
}

pub async fn send_diff(bot: &Bot, chat_id: i64, ctx: &DiffContext<'_>) -> Result<(), TelegramError> {
    // Drops only (caller already filters); keep the down arrow.
    let arrow = "🔻";
    let label = escape_md_v2(ctx.label);
    let new_p = format!("${:.2}", ctx.diff.new_price);
    let old_p = format!("${:.2}", ctx.diff.old_price);
    let pct = format!("{:+.1}%", ctx.diff.delta_pct);
    let mut text = format!(
        "{arrow} *{label}*\n*{new}* — was {old} \\({pct}\\)",
        new = escape_md_v2(&new_p),
        old = escape_md_v2(&old_p),
        pct = escape_md_v2(&pct),
    );

    if let Some(msrp_label) = ctx.msrp_label {
        if let Some(msrp_val) = parse_money(msrp_label) {
            if msrp_val > ctx.diff.new_price {
                let savings_pct = ((msrp_val - ctx.diff.new_price) / msrp_val) * 100.0;
                let line = format!(
                    "{}% off MSRP {}",
                    savings_pct.round() as i64,
                    msrp_label,
                );
                text.push('\n');
                text.push_str(&escape_md_v2(&line));
            }
        }
    }

    bot.send_message(ChatId(chat_id), text)
        .parse_mode(ParseMode::MarkdownV2)
        .await?;
    Ok(())
}

pub async fn send_text(bot: &Bot, chat_id: i64, text: impl Into<String>) -> Result<(), TelegramError> {
    bot.send_message(ChatId(chat_id), text.into()).await?;
    Ok(())
}

/// Strip leading currency symbol and parse the number. Handles `"$135.00"`, `"$ 135"`,
/// `"135.00 USD"`. Returns None on anything ambiguous.
fn parse_money(s: &str) -> Option<f64> {
    let trimmed: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    let normalized = trimmed.replace(',', "");
    normalized.parse().ok()
}

/// Telegram MarkdownV2 requires escaping these characters everywhere outside
/// code blocks: _ * [ ] ( ) ~ ` > # + - = | { } . !
fn escape_md_v2(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if matches!(
            c,
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#'
                | '+' | '-' | '=' | '|' | '{' | '}' | '.' | '!'
                | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_basics() {
        assert_eq!(escape_md_v2("Drinks (Premium)"), "Drinks \\(Premium\\)");
        assert_eq!(escape_md_v2("$87.99"), "$87\\.99");
        assert_eq!(escape_md_v2("safe text"), "safe text");
    }

    #[test]
    fn money_parses() {
        assert_eq!(parse_money("$135.00"), Some(135.0));
        assert_eq!(parse_money("135"), Some(135.0));
        assert_eq!(parse_money("1,234.50 USD"), Some(1234.50));
    }
}
