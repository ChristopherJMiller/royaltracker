//! Live Princess login smoke test. Skips unless both env vars are set. Run ONCE
//! (unknown per-IP auth rate limit; do not loop):
//!
//!     PRINCESS_LOGIN=you@example.com PRINCESS_PASSWORD='...' \
//!     cargo test -p royaltracker-api --test princess_login_smoke -- --nocapture

use royaltracker_api::princess::PrincessAccountClient;

#[tokio::test]
async fn princess_login_returns_booking_context() {
    let (Ok(login), Ok(password)) = (
        std::env::var("PRINCESS_LOGIN"),
        std::env::var("PRINCESS_PASSWORD"),
    ) else {
        eprintln!("skipping princess login smoke: PRINCESS_LOGIN / PRINCESS_PASSWORD not set");
        return;
    };
    let client = PrincessAccountClient::new().expect("build client");
    let ctx = client
        .login_and_context(&login, &password)
        .await
        .expect("login should succeed")
        .expect("account should have a current booking");
    assert_eq!(ctx.booking_id.len(), 6, "booking id should be 6 chars");
    eprintln!(
        "ok: booking {} ship {} voyage {} nights {:?} sail {} {}",
        ctx.booking_id, ctx.ship_code, ctx.voyage_code, ctx.nights, ctx.sail_date, ctx.currency
    );
    // Best-effort in the client, so report it rather than fail on it.
    match &ctx.cabin {
        Some(c) => eprintln!("ok: cabin {} deck {:?} category {:?}", c.number, c.deck_name, c.category),
        None => eprintln!("warn: no cabin from booking/summary (unassigned, or the call failed; see logs)"),
    }
}
