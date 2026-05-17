//! Live login smoke test. Gated on env vars so CI without RCG credentials skips
//! cleanly. Run locally before each deploy:
//!
//!     ROYALTRACKER_TEST_USERNAME=you@example.com \
//!     ROYALTRACKER_TEST_PASSWORD='...' \
//!     ROYALTRACKER_TEST_BASIC_AUTH_B64='...' \
//!     cargo test -p royaltracker-api --test login_smoke -- --nocapture
//!
//! Asserts the Akamai-fronted OAuth endpoint accepts our request shape for both
//! brands. This is the regression detector promised in the 2026-05-17 incident
//! ticket — next time Bot Manager bumps, this test starts failing before the
//! cron does.

use royaltracker_api::{CruiseClient, CruiseClientConfig};
use royaltracker_types::Brand;

fn creds() -> Option<(String, String, String)> {
    Some((
        std::env::var("ROYALTRACKER_TEST_USERNAME").ok()?,
        std::env::var("ROYALTRACKER_TEST_PASSWORD").ok()?,
        std::env::var("ROYALTRACKER_TEST_BASIC_AUTH_B64").ok()?,
    ))
}

async fn try_brand(brand: Brand) {
    let Some((u, p, ba)) = creds() else {
        eprintln!("skipping {brand}: credentials env vars not set");
        return;
    };
    let cfg = CruiseClientConfig::web(brand, u, p, ba);
    let client = CruiseClient::new(cfg).expect("build client");
    let token = client.login().await.expect("login should succeed");
    assert!(!token.access_token.is_empty(), "empty access token");
    assert!(!token.account_id.is_empty(), "empty account id");
}

#[tokio::test]
async fn login_royal() {
    try_brand(Brand::Royal).await;
}

#[tokio::test]
async fn login_celebrity() {
    try_brand(Brand::Celebrity).await;
}
