use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::handlers;
use crate::AppState;

/// Build the API router with all routes.
pub fn build_router(state: Arc<AppState>) -> Router {
    let api_routes = Router::new()
        .route("/search", post(handlers::search::search))
        .route("/top-searches", get(handlers::top_searches::top_searches))
        .route("/autocomplete", get(handlers::autocomplete::autocomplete))
        .route("/feedback", post(handlers::feedback::create_feedback));

    Router::new()
        .nest("/api/v1", api_routes)
        .route("/health", get(handlers::health::health))
        .route("/ready", get(handlers::health::ready))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}
