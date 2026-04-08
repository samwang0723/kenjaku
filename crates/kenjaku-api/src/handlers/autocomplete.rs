use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use tracing::{debug, error};

use crate::AppState;
use crate::dto::response::{ApiResponse, AutocompleteResponseDto, BlendedItemDto};
use crate::extractors::ResolvedLocale;

/// Maximum autocomplete limit.
const MAX_LIMIT: usize = 50;

#[derive(Deserialize)]
pub struct AutocompleteQuery {
    pub q: String,
    /// Explicit locale override handled by `ResolvedLocale`. Kept here only so
    /// axum does not reject unknown query params.
    #[allow(dead_code)]
    pub locale: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub session_id: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    10
}

/// GET /api/v1/autocomplete
///
/// Uses the `ResolvedLocale` extractor for locale resolution, then delegates
/// to the existing autocomplete service. Dev-1 swaps the backing call to
/// `SuggestionService::autocomplete` during peer review without changing the
/// DTO shape.
pub async fn autocomplete(
    State(state): State<Arc<AppState>>,
    resolved: ResolvedLocale,
    Query(params): Query<AutocompleteQuery>,
) -> Json<ApiResponse<AutocompleteResponseDto>> {
    if params.q.is_empty() {
        return Json(ApiResponse::err(
            "Query parameter 'q' is required".to_string(),
        ));
    }

    let limit = params.limit.min(MAX_LIMIT);
    let locale_str = resolved.locale_str();
    debug!(
        locale = %locale_str,
        source = resolved.source_str(),
        "autocomplete resolved locale"
    );

    match state
        .autocomplete_service
        .suggest(&params.q, &locale_str, limit)
        .await
    {
        Ok(suggestions) => {
            let items: Vec<BlendedItemDto> = suggestions
                .iter()
                .map(|q| BlendedItemDto {
                    query: q.clone(),
                    source: "crowdsourced".to_string(),
                    score: None,
                    weight: None,
                })
                .collect();
            Json(ApiResponse::ok(AutocompleteResponseDto {
                suggestions,
                items,
                resolved_locale: locale_str,
                resolved_locale_source: resolved.source_str().to_string(),
            }))
        }
        Err(e) => {
            error!(error = %e, "Autocomplete failed");
            Json(ApiResponse::err(e.user_message().to_string()))
        }
    }
}
