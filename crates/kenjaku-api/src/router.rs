use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::routing::{get, post};
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::SmartIpKeyExtractor;
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::AppState;
use crate::handlers;

/// Build the API router with all routes, rate limiting, and security layers.
#[allow(deprecated)] // TimeoutLayer::new deprecated in tower-http 0.6, replacement API differs
pub fn build_router(state: Arc<AppState>) -> Router {
    // Rate limiter: 60 requests per minute per IP
    // SmartIpKeyExtractor checks X-Forwarded-For, X-Real-Ip, then peer addr
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(1)
            .burst_size(60)
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .expect("failed to build rate limiter config"),
    );

    // API routes — rate limited
    let api_routes = Router::new()
        .route("/search", post(handlers::search::search))
        .route("/top-searches", get(handlers::top_searches::top_searches))
        .route("/autocomplete", get(handlers::autocomplete::autocomplete))
        .route("/feedback", post(handlers::feedback::create_feedback))
        .layer(GovernorLayer {
            config: governor_conf,
        });

    // Health routes — no rate limiting (for load balancer probes)
    let health_routes = Router::new()
        .route("/health", get(handlers::health::health))
        .route("/ready", get(handlers::health::ready));

    Router::new()
        .nest("/api/v1", api_routes)
        .merge(health_routes)
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .layer(RequestBodyLimitLayer::new(1024 * 64)) // 64 KB
        .layer(TraceLayer::new_for_http())
        .layer(
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
                    http::header::ACCEPT_LANGUAGE,
                    http::HeaderName::from_static("x-session-id"),
                ])),
        )
        .with_state(state)
}
