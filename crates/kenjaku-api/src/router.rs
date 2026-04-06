use std::sync::Arc;
use std::time::Duration;

use axum::routing::{get, post};
use axum::Router;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::GovernorLayer;
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::handlers;
use crate::AppState;

/// Build the API router with all routes, rate limiting, and security layers.
#[allow(deprecated)] // TimeoutLayer::new deprecated in tower-http 0.6, replacement API differs
pub fn build_router(state: Arc<AppState>) -> Router {
    // Rate limiter: 60 requests per minute per IP
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(1)
            .burst_size(60)
            .finish()
            .expect("failed to build rate limiter config"),
    );

    let api_routes = Router::new()
        .route("/search", post(handlers::search::search))
        .route("/top-searches", get(handlers::top_searches::top_searches))
        .route("/autocomplete", get(handlers::autocomplete::autocomplete))
        .route("/feedback", post(handlers::feedback::create_feedback));

    Router::new()
        .nest("/api/v1", api_routes)
        .route("/health", get(handlers::health::health))
        .route("/ready", get(handlers::health::ready))
        .layer(GovernorLayer {
            config: governor_conf,
        })
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .layer(RequestBodyLimitLayer::new(1024 * 64)) // 64 KB
        .layer(TraceLayer::new_for_http())
        .layer(
            // CORS: Allow any origin for now. Restrict to specific origins
            // via config when deploying behind a known frontend domain.
            CorsLayer::new()
                .allow_origin(AllowOrigin::any())
                .allow_methods(AllowMethods::list([
                    http::Method::GET,
                    http::Method::POST,
                    http::Method::OPTIONS,
                ]))
                .allow_headers(AllowHeaders::list([
                    http::header::CONTENT_TYPE,
                    http::header::AUTHORIZATION,
                    http::header::ACCEPT,
                ])),
        )
        .with_state(state)
}
