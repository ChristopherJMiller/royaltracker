//! Live guest check of the Princess SFCC client (no credentials).
//! nix develop -c cargo run -p royaltracker-api --example probe_princess

use royaltracker_api::princess::PrincessGuestClient;
use royaltracker_api::PacingConfig;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pacing = PacingConfig {
        min_interval: Duration::from_millis(800),
        jitter: (Duration::from_millis(100), Duration::from_millis(400)),
        ..Default::default()
    };
    let c = PrincessGuestClient::new(pacing)?;
    let exc = c.fetch_excursions("4702", "USD").await?;
    println!("excursions: {}", exc.len());
    for p in exc.iter().take(3) {
        println!("  {} {:?} {:?} {:?}", p.product_code, p.title, p.promo_label, p.unit_label);
    }
    let extras = c.fetch_extras("USD").await?;
    println!("extras: {}", extras.len());
    for p in extras.iter().filter(|p| p.category_id == "wifi") {
        println!("  {} {:?} base {:?} {:?}", p.product_code, p.promo_label, p.base_label, p.unit_label);
    }
    Ok(())
}
