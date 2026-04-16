pub mod dto;
pub mod extractors;
pub mod handlers;
pub mod middleware;
pub mod router;

use kenjaku_core::config::{RateLimitConfig, TenancyConfig};
use kenjaku_infra::auth::JwtValidator;
use kenjaku_infra::postgres::TenantsCache;
use kenjaku_infra::qdrant::QdrantClient;
use kenjaku_infra::redis::RedisClient;
use std::sync::Arc;

use kenjaku_service::autocomplete::AutocompleteService;
use kenjaku_service::feedback::FeedbackService;
use kenjaku_service::search::SearchService;
use kenjaku_service::suggestion::SuggestionService;
use kenjaku_service::trending::TrendingService;
use sqlx::PgPool;

/// Shared application state passed to all handlers.
///
/// Phase 3e: tenancy is always on. `jwt_validator` is always present
/// (not optional). Handlers access the tenant via the `TenantContext`
/// extractor, not directly from AppState.
pub struct AppState {
    pub search_service: SearchService,
    pub trending_service: TrendingService,
    pub autocomplete_service: AutocompleteService,
    pub suggestion_service: Arc<SuggestionService>,
    pub feedback_service: FeedbackService,
    pub qdrant: QdrantClient,
    pub redis: RedisClient,
    pub pg_pool: PgPool,
    /// Read-only snapshot of `tenants` table loaded at startup.
    pub tenants_cache: Arc<TenantsCache>,
    /// JWT validator — always constructed at startup (Phase 3e).
    pub jwt_validator: Arc<JwtValidator>,
    /// Clone of `AppConfig.tenancy` — used for collection name template.
    pub tenancy_config: TenancyConfig,
    /// Clone of `AppConfig.rate_limit` — read by the
    /// `TenantPrincipalIpExtractor` to decide key shape.
    pub rate_limit_config: RateLimitConfig,
}
