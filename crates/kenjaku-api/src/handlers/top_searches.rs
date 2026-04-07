use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use tracing::error;

use crate::AppState;
use crate::dto::response::{ApiResponse, TopSearchDto};

use kenjaku_core::types::trending::TrendingPeriod;

/// Maximum result limit for top searches.
const MAX_LIMIT: usize = 100;

#[derive(Deserialize)]
pub struct TopSearchesQuery {
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default = "default_period")]
    pub period: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_locale() -> String {
    "en".to_string()
}

fn default_period() -> String {
    "daily".to_string()
}

fn default_limit() -> usize {
    20
}

/// GET /api/v1/top-searches
pub async fn top_searches(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TopSearchesQuery>,
) -> Json<ApiResponse<Vec<TopSearchDto>>> {
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

    match state
        .trending_service
        .get_top_searches(&params.locale, &period, limit)
        .await
    {
        Ok(queries) => {
            let dtos: Vec<TopSearchDto> = queries
                .into_iter()
                .map(|q| TopSearchDto {
                    query: q.query,
                    count: q.search_count,
                })
                .collect();
            Json(ApiResponse::ok(dtos))
        }
        Err(e) => {
            error!(error = %e, "Top searches failed");
            Json(ApiResponse::err(e.user_message().to_string()))
        }
    }
}
