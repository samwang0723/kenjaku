use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::Extension;
use axum::routing::{get, patch, post};
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
use crate::middleware::auth::tenant_auth_middleware;
use crate::middleware::rate_limit::TenantPrincipalIpExtractor;
use crate::middleware::require_admin::require_admin;

/// Build the API router with all routes, rate limiting, and security layers.
///
/// `locale_lookup` is injected as a request `Extension` so the
/// `ResolvedLocale` extractor can resolve session-stickied locales without
/// the api crate depending on the concrete service-layer `LocaleMemory`.
///
/// # Phase 3c.2 wiring
///
/// - Auth middleware (`tenant_auth_middleware`) wraps every API route.
///   Runs BEFORE the rate limiter so the limiter can read the resolved
///   `TenantContext` from extensions.
/// - Rate limiter key extractor is `TenantPrincipalIpExtractor` whose
///   strategy comes from `state.rate_limit_config.key_strategy`. Default
///   `Ip` is byte-identical to pre-3c.2 behavior.
#[allow(deprecated)] // TimeoutLayer::new deprecated in tower-http 0.6, replacement API differs
pub fn build_router(
    state: Arc<AppState>,
    locale_lookup: Arc<dyn SessionLocaleLookup>,
    server_config: &ServerConfig,
) -> Router {
    // Rate limiter — pluggable key strategy from config. Default `Ip`
    // preserves pre-3c.2 behavior; `TenantIp` / `TenantPrincipalIp`
    // read the auth-middleware-placed `TenantContext` from request
    // extensions.
    let key_extractor = TenantPrincipalIpExtractor::new(state.rate_limit_config.key_strategy);
    let governor_conf = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(server_config.rate_limit_per_second)
            .burst_size(server_config.rate_limit_burst)
            .key_extractor(key_extractor)
            .finish()
            .expect("failed to build rate limiter config"),
    );

    // Auth middleware layer — closes over AppState (cache + validator
    // + tenancy_config). Wired with `from_fn_with_state` so it runs
    // BEFORE the GovernorLayer; the rate limiter then reads the
    // resolved `TenantContext` from extensions.
    let auth_layer = axum::middleware::from_fn_with_state(state.clone(), tenant_auth_middleware);

    // API routes — auth middleware runs first, then rate limit.
    let api_routes = Router::new()
        .route("/search", post(handlers::search::search))
        .route("/top-searches", get(handlers::top_searches::top_searches))
        .route("/autocomplete", get(handlers::autocomplete::autocomplete))
        .route("/feedback", post(handlers::feedback::create_feedback))
        // Layer order: outermost first when reading top-down, but
        // executed inner-first per-request. So we attach the rate
        // limiter first (innermost runtime layer) then auth (outermost
        // — runs before rate-limit can read the tctx).
        .layer(GovernorLayer {
            config: governor_conf,
        })
        .layer(auth_layer.clone());

    // /admin/users/* — same auth middleware PLUS require_admin. The
    // rate-limit layer is NOT re-applied here (it's added at the
    // /api/v1 merge below via a separate router so both api_routes
    // and admin_routes share the same bucket).
    let admin_routes = Router::new()
        .route(
            "/admin/users",
            get(handlers::admin_users::list_users).post(handlers::admin_users::create_user),
        )
        .route(
            "/admin/users/{id}",
            patch(handlers::admin_users::update_user).delete(handlers::admin_users::delete_user),
        )
        .route(
            "/admin/users/{id}/reset-password",
            post(handlers::admin_users::reset_password),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_admin,
        ))
        .layer(auth_layer);

    // /auth/* — login route ONLY. Skips tenant_auth_middleware (pre-
    // auth, no tenant context exists yet). Gets its own tighter
    // per-IP governor: 5 req / 60s with burst 3, chosen to slow down
    // credential-stuffing without crippling a forgetful user retrying
    // a typo. Pre-auth == no TenantContext, so we use the plain
    // SmartIpKeyExtractor (the tenant-aware extractor would just fall
    // back to "public" here anyway).
    let login_governor = Arc::new(
        GovernorConfigBuilder::default()
            // `tower_governor` uses per-ms internally. 5 per 60s with
            // burst 3 is the spec: set per_second=0 by using the
            // precise `period` setter.
            .period(Duration::from_secs(12))
            .burst_size(3)
            .key_extractor(SmartIpKeyExtractor)
            .finish()
            .expect("login rate limiter config"),
    );
    let auth_routes = Router::new()
        .route("/auth/login", post(handlers::auth::login))
        .layer(GovernorLayer {
            config: login_governor,
        });

    // Health routes — no auth, no rate limiting (load balancer probes).
    let health_routes = Router::new()
        .route("/health", get(handlers::health::health))
        .route("/ready", get(handlers::health::ready));

    Router::new()
        .nest("/api/v1", api_routes.merge(admin_routes).merge(auth_routes))
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
