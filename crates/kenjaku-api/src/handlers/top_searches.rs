use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use tracing::{debug, error};

use crate::AppState;
use crate::dto::response::{ApiResponse, BlendedItemDto, TopSearchesResponse};
use crate::extractors::ResolvedLocale;

use kenjaku_core::types::trending::TrendingPeriod;

/// Maximum result limit for top searches.
const MAX_LIMIT: usize = 100;

#[derive(Deserialize)]
pub struct TopSearchesQuery {
    /// Explicit locale override — when present and parseable it wins over
    /// session memory and `Accept-Language`. Parsed by the extractor, kept
    /// here purely so axum doesn't reject unknown query params.
    #[allow(dead_code)]
    pub locale: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub session_id: Option<String>,
    #[serde(default = "default_period")]
    pub period: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_period() -> String {
    "daily".to_string()
}

fn default_limit() -> usize {
    20
}

/// GET /api/v1/top-searches
///
/// Locale is resolved via the `ResolvedLocale` extractor (chain:
/// `?locale=` → session memory → `Accept-Language` → `en`). The handler
/// currently delegates to `TrendingService`; dev-1's `SuggestionService` takes
/// over the blending path during peer-review merge (per architect.md sync
/// contract — the DTO shape does not change).
pub async fn top_searches(
    State(state): State<Arc<AppState>>,
    resolved: ResolvedLocale,
    Query(params): Query<TopSearchesQuery>,
) -> Json<ApiResponse<TopSearchesResponse>> {
    let period: TrendingPeriod = match params.period.parse() {
        Ok(p) => p,
        Err(_) => {
            return Json(ApiResponse::err(format!(
                "Invalid period '{}'. Supported: daily, weekly",
                params.period
            )));
        }
    };

    let limit = params.limit.min(MAX_LIMIT);
    let locale_str = resolved.locale_str();
    debug!(
        locale = %locale_str,
        source = resolved.source_str(),
        "top_searches resolved locale"
    );

    match state
        .trending_service
        .get_top_searches(&locale_str, &period, limit)
        .await
    {
        Ok(queries) => {
            let items: Vec<BlendedItemDto> = queries
                .into_iter()
                .map(|q| BlendedItemDto {
                    query: q.query,
                    source: "crowdsourced".to_string(),
                    score: Some(q.search_count as f64),
                    weight: None,
                })
                .collect();
            Json(ApiResponse::ok(TopSearchesResponse {
                items,
                resolved_locale: locale_str,
                resolved_locale_source: resolved.source_str().to_string(),
            }))
        }
        Err(e) => {
            error!(error = %e, "Top searches failed");
            Json(ApiResponse::err(e.user_message().to_string()))
        }
    }
}
