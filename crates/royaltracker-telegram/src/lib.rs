use royaltracker_types::{format_money, Diff};
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
    /// Which sailing this price is for, e.g.
    /// "Wonder of the Seas · 3 nights · Sep 4, 2026 · cabin 9434". Rendered as a
    /// subtitle so alerts across multiple bookings are distinguishable at a glance.
    pub itinerary: Option<&'a str>,
    /// ISO currency code for prices; `None` renders as USD ("$").
    pub currency: Option<&'a str>,
    /// Pricing unit shown after the new price, e.g. "per person / day".
    /// `None` leaves the message unchanged.
    pub unit_label: Option<&'a str>,
    /// Set when this is a booked item now cheaper than what was paid: the
    /// message compares with what was paid instead of the last price.
    pub paid: Option<PaidComparison<'a>>,
}

/// Today's price against what the cabin paid, pre-formatted. Labels are per
/// person when everyone paid the same, otherwise cabin totals (`group`).
#[derive(Debug, Clone, Copy)]
pub struct PaidComparison<'a> {
    pub now_label: &'a str,
    pub paid_label: &'a str,
    /// "per day" / "per night"; `None` for a one-off.
    pub unit: Option<&'a str>,
    /// `Some(n)`: guests paid different amounts, labels are totals for n.
    pub group: Option<u32>,
    pub savings_label: &'a str,
    /// e.g. "2 guests × 7 days × $4.00"; only when everyone paid the same.
    pub breakdown: Option<&'a str>,
}

/// The "cheaper than you paid" message.
fn render_paid(ctx: &DiffContext<'_>, p: &PaidComparison<'_>) -> String {
    let mut text = format!("🔻 *{} is cheaper than you paid*", escape_md_v2(ctx.label));
    if let Some(itin) = ctx.itinerary {
        text.push_str(&format!("\n🚢 {}", escape_md_v2(itin)));
    }
    let unit = p.unit.map(|u| format!(" {u}")).unwrap_or_default();
    let group = p.group.map(|n| format!(" for {n}")).unwrap_or_default();
    text.push_str(&format!(
        "\n*{}*{} — you paid {}",
        escape_md_v2(p.now_label),
        escape_md_v2(&format!("{unit}{group}")),
        escape_md_v2(p.paid_label),
    ));
    let line = match (p.group, p.breakdown) {
        (Some(_), _) => format!("Rebook everyone to save {}", p.savings_label),
        (None, Some(b)) => format!("Rebook to save {} ({b})", p.savings_label),
        (None, None) => format!("Rebook to save {}", p.savings_label),
    };
    text.push('\n');
    text.push_str(&escape_md_v2(&line));
    text
}

/// Build the MarkdownV2 message text for a price drop.
pub fn render_diff(ctx: &DiffContext<'_>) -> String {
    if let Some(p) = &ctx.paid {
        return render_paid(ctx, p);
    }
    // Drops only (caller already filters); keep the down arrow.
    let arrow = "🔻";
    let label = escape_md_v2(ctx.label);
    let new_p = format_money(ctx.diff.new_price, ctx.currency);
    let old_p = format_money(ctx.diff.old_price, ctx.currency);
    let pct = format!("{:+.1}%", ctx.diff.delta_pct);
    let mut text = format!("{arrow} *{label}*");
    if let Some(itin) = ctx.itinerary {
        text.push_str(&format!("\n🚢 {}", escape_md_v2(itin)));
    }
    let unit = ctx
        .unit_label
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .map(|u| format!(" {}", escape_md_v2(u)))
        .unwrap_or_default();
    text.push_str(&format!(
        "\n*{new}*{unit} — was {old} \\({pct}\\)",
        new = escape_md_v2(&new_p),
        old = escape_md_v2(&old_p),
        pct = escape_md_v2(&pct),
    ));

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

    text
}

pub async fn send_diff(bot: &Bot, chat_id: i64, ctx: &DiffContext<'_>) -> Result<(), TelegramError> {
    let text = render_diff(ctx);
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

    fn paid<'a>(
        now: &'a str,
        paid: &'a str,
        unit: Option<&'a str>,
        group: Option<u32>,
        savings: &'a str,
        breakdown: Option<&'a str>,
    ) -> PaidComparison<'a> {
        PaidComparison { now_label: now, paid_label: paid, unit, group, savings_label: savings, breakdown }
    }

    #[test]
    fn paid_message_equal_guests() {
        let d = Diff::from_prices(1, 145.98, 137.98);
        let text = render_diff(&DiffContext {
            itinerary: Some("Harmony of the Seas · 7 nights · Mar 14, 2027"),
            paid: Some(paid("$68.99", "$72.99", Some("per day"), None, "$56.00", Some("2 guests × 7 days × $4.00"))),
            ..ctx("Deluxe Beverage Package", &d)
        });
        assert_eq!(
            text,
            "🔻 *Deluxe Beverage Package is cheaper than you paid*\n🚢 Harmony of the Seas · 7 nights · Mar 14, 2027\n*$68\\.99* per day — you paid $72\\.99\nRebook to save $56\\.00 \\(2 guests × 7 days × $4\\.00\\)"
        );
    }

    #[test]
    fn paid_message_unequal_guests() {
        let d = Diff::from_prices(1, 145.98, 137.98);
        let text = render_diff(&DiffContext {
            paid: Some(paid("$137.98", "$145.98", Some("per day"), Some(2), "$56.00", None)),
            ..ctx("Deluxe Beverage Package", &d)
        });
        assert!(
            text.ends_with("\n*$137\\.98* per day for 2 — you paid $145\\.98\nRebook everyone to save $56\\.00"),
            "{text}"
        );
    }

    #[test]
    fn paid_message_one_off() {
        let d = Diff::from_prices(1, 219.99, 189.99);
        let text = render_diff(&DiffContext {
            msrp_label: Some("$249.00"),
            paid: Some(paid("$189.99", "$219.99", None, None, "$30.00", None)),
            ..ctx("Day Pass", &d)
        });
        assert_eq!(text, "🔻 *Day Pass is cheaper than you paid*\n*$189\\.99* — you paid $219\\.99\nRebook to save $30\\.00");
    }

    fn ctx<'a>(label: &'a str, diff: &'a Diff) -> DiffContext<'a> {
        DiffContext { label, diff, msrp_label: None, itinerary: None, currency: None, unit_label: None, paid: None }
    }

    #[test]
    fn unit_label_follows_new_price() {
        let d = Diff::from_prices(1, 30.0, 24.94);
        let text = render_diff(&DiffContext {
            currency: Some("AUD"),
            unit_label: Some("per person / day"),
            ..ctx("Classic Soda", &d)
        });
        assert!(
            text.contains("*AUD 24\\.94* per person / day — was AUD 30\\.00 \\(\\-16\\.9%\\)"),
            "{text}"
        );
        // Empty unit renders as no unit.
        let blank = render_diff(&DiffContext { unit_label: Some("  "), ..ctx("X", &d) });
        assert_eq!(blank, render_diff(&ctx("X", &d)));
    }

    #[test]
    fn aud_drop_renders_currency_code() {
        let d = Diff::from_prices(1, 30.0, 24.94);
        let text = render_diff(&DiffContext { currency: Some("AUD"), ..ctx("Classic Soda", &d) });
        assert!(text.contains("AUD 24\\.94"), "{text}");
        assert!(!text.contains("$24"));
    }

    #[test]
    fn usd_default_unchanged() {
        let d = Diff::from_prices(1, 30.0, 24.94);
        assert!(render_diff(&ctx("X", &d)).contains("$24\\.94"));
    }

    #[test]
    fn rcg_full_message_byte_identical() {
        let d = Diff::from_prices(1, 107.99, 87.99);
        let text = render_diff(&DiffContext {
            msrp_label: Some("$135.00"),
            itinerary: Some("Wonder of the Seas · 3 nights · Sep 4, 2026 · cabin 9434 (assigned)"),
            ..ctx("Deluxe Beverage Package (Adult)", &d)
        });
        assert_eq!(
            text,
            "🔻 *Deluxe Beverage Package \\(Adult\\)*\n\
             🚢 Wonder of the Seas · 3 nights · Sep 4, 2026 · cabin 9434 \\(assigned\\)\n\
             *$87\\.99* — was $107\\.99 \\(\\-18\\.5%\\)\n\
             35% off MSRP $135\\.00"
        );
    }

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
