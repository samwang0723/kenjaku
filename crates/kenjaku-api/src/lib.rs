pub mod dto;
pub mod extractors;
pub mod handlers;
pub mod middleware;
pub mod router;

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
pub struct AppState {
    pub search_service: SearchService,
    pub trending_service: TrendingService,
    pub autocomplete_service: AutocompleteService,
    pub suggestion_service: Arc<SuggestionService>,
    pub feedback_service: FeedbackService,
    pub qdrant: QdrantClient,
    pub redis: RedisClient,
    pub pg_pool: PgPool,
}
