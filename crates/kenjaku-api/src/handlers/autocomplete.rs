use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use tracing::error;

use crate::dto::response::{ApiResponse, AutocompleteResponseDto};
use crate::AppState;

/// Maximum autocomplete limit.
const MAX_LIMIT: usize = 50;

#[derive(Deserialize)]
pub struct AutocompleteQuery {
    pub q: String,
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_locale() -> String {
    "en".to_string()
}

fn default_limit() -> usize {
    10
}

/// GET /api/v1/autocomplete
pub async fn autocomplete(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AutocompleteQuery>,
) -> Json<ApiResponse<AutocompleteResponseDto>> {
    if params.q.is_empty() {
        return Json(ApiResponse::err("Query parameter 'q' is required".to_string()));
    }

    let limit = params.limit.min(MAX_LIMIT);

    match state
        .autocomplete_service
        .suggest(&params.q, &params.locale, limit)
        .await
    {
        Ok(suggestions) => Json(ApiResponse::ok(AutocompleteResponseDto { suggestions })),
        Err(e) => {
            error!(error = %e, "Autocomplete failed");
            Json(ApiResponse::err(e.user_message().to_string()))
        }
    }
}
