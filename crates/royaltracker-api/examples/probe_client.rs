//! Exercise the real PublicClient end-to-end against a live sailing.
//! nix develop -c cargo run -p royaltracker-api --example probe_client

use royaltracker_api::{PacingConfig, PublicClient, PublicClientConfig, PublicFetch};
use royaltracker_types::Brand;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Fast pacing for a quick probe (prod uses the 3s/2-8s defaults).
    let pacing = PacingConfig {
        min_interval: Duration::from_millis(300),
        jitter: (Duration::from_millis(100), Duration::from_millis(400)),
        ..Default::default()
    };
    let ua = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36".to_string();
    let client = PublicClient::new(PublicClientConfig::new(ua, pacing))?;

    for (brand, ship, sail) in [
        (Brand::Royal, "OA", "2027-05-07"),   // far, active promos
        (Brand::Royal, "OA", "2028-01-14"),   // too far, planner not open
    ] {
        let d = chrono::NaiveDate::parse_from_str(sail, "%Y-%m-%d")?;
        print!("\n== {ship} {sail} == ");
        match client.fetch_public_products(brand, ship, d).await? {
            PublicFetch::PlannerNotOpen => println!("PlannerNotOpen"),
            PublicFetch::Products(ps) => {
                let with_promo = ps.iter().filter(|p| p.promo_cents.is_some()).count();
                println!("{} products, {} with a promo", ps.len(), with_promo);
                for p in ps.iter().filter(|p| p.category_id == "beverage").take(5) {
                    println!(
                        "   promo={:>8?} base={:>8?}  {}",
                        p.promo_dollars(),
                        p.base_dollars(),
                        p.title.as_deref().unwrap_or("?")
                    );
                }
            }
        }
    }
    Ok(())
}
