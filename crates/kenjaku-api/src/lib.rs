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
/// Phase 3c.2 additions (`tenants_cache`, `jwt_validator`,
/// `tenancy_config`, `rate_limit_config`) power the new auth
/// middleware + per-tenant rate-limit extractor. Handlers do NOT
/// read them directly — the `TenantContext` extractor is the typed
/// handle. Keeping these in `AppState` lets the middleware close
/// over them via `axum::middleware::from_fn_with_state`.
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
    /// Present when `tenancy.enabled=true`; `None` when disabled.
    /// The auth middleware NEVER reads this field in the
    /// `enabled=false` branch — verified by
    /// `disabled_mode_never_invokes_validator` test.
    pub jwt_validator: Option<Arc<JwtValidator>>,
    /// Clone of `AppConfig.tenancy` — the middleware reads `.enabled`
    /// + `.jwt` on every request. Cheap (2 String fields + 2 bool/enum).
    pub tenancy_config: TenancyConfig,
    /// Clone of `AppConfig.rate_limit` — read by the
    /// `TenantPrincipalIpExtractor` to decide key shape.
    pub rate_limit_config: RateLimitConfig,
}
