//! Smoke-test harness for the public tier: serve just the public_router against
//! a scratch SQLite DB. Run:
//!   nix develop -c cargo run -p royaltracker-web --example serve_public --features sqlite

use royaltracker_api::{PacingConfig, PublicClient, PublicClientConfig, DEFAULT_USER_AGENT};
use royaltracker_storage::{connect, PriceRepo};
use royaltracker_web::{public_router, PublicState};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = std::env::var("DB").unwrap_or_else(|_| "sqlite::memory:".into());
    let repo = connect(&db).await?;
    repo.migrate().await?;
    let client = PublicClient::new(PublicClientConfig::new(
        DEFAULT_USER_AGENT.to_string(),
        PacingConfig::default(),
    ))?;
    let state = PublicState {
        repo: Arc::new(repo),
        public_client: Arc::new(client),
    };
    let addr: std::net::SocketAddr = "127.0.0.1:8099".parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("serving public tier on http://{addr}");
    axum::serve(listener, public_router(state)).await?;
    Ok(())
}
