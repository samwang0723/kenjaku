use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::Extension;
use axum::routing::{get, post};
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::SmartIpKeyExtractor;
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use kenjaku_core::config::ServerConfig;

use crate::AppState;
use crate::extractors::SessionLocaleLookup;
use crate::handlers;

/// Build the API router with all routes, rate limiting, and security layers.
///
/// `locale_lookup` is injected as a request `Extension` so the
/// `ResolvedLocale` extractor can resolve session-stickied locales without
/// the api crate depending on the concrete service-layer `LocaleMemory`.
#[allow(deprecated)] // TimeoutLayer::new deprecated in tower-http 0.6, replacement API differs
pub fn build_router(
    state: Arc<AppState>,
    locale_lookup: Arc<dyn SessionLocaleLookup>,
    server_config: &ServerConfig,
) -> Router {
    // Rate limiter: configurable requests per second per IP
    // SmartIpKeyExtractor checks X-Forwarded-For, X-Real-Ip, then peer addr
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(server_config.rate_limit_per_second)
            .burst_size(server_config.rate_limit_burst)
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
        .layer(TimeoutLayer::new(Duration::from_secs(
            server_config.request_timeout_secs,
        )))
        .layer(RequestBodyLimitLayer::new(server_config.body_limit_bytes))
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
        .layer(Extension(locale_lookup))
        .with_state(state)
}
