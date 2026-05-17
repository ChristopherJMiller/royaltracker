mod auth;
mod handlers;
mod state;
mod static_assets;

pub use auth::{verify_init_data, AuthError, AuthedUser, TgAuthOnly};
pub use state::AppState;

use axum::routing::{delete, get, post, put};
use axum::Router;
use std::sync::Arc;
use tower_http::compression::CompressionLayer;

pub fn router(state: AppState) -> Router {
    Router::new()
        // API
        .route("/api/register", post(handlers::register))
        .route("/api/refresh", post(handlers::refresh))
        .route("/api/catalog/refresh", post(handlers::refresh_catalog))
        .route("/api/me", get(handlers::get_me))
        .route("/api/bookings", get(handlers::list_bookings))
        .route("/api/catalog/search", get(handlers::search_catalog))
        .route("/api/catalog/browse", get(handlers::browse_catalog))
        .route("/api/watched", get(handlers::list_watched))
        .route("/api/watched", post(handlers::add_watched))
        .route("/api/watched/:id", delete(handlers::remove_watched))
        .route("/api/watched/:id/alert", put(handlers::set_alert))
        .route("/api/history/:watched_id", get(handlers::history))
        // Static frontend (catch-all so deep links work)
        .fallback(static_assets::serve)
        .layer(CompressionLayer::new().gzip(true))
        .with_state(Arc::new(state))
}
