use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use tracing::error;

use crate::dto::response::{ApiResponse, TopSearchDto};
use crate::AppState;

use kenjaku_core::types::trending::TrendingPeriod;

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
        Err(e) => {
            return Json(ApiResponse::err(e.to_string()));
        }
    };

    match state
        .trending_service
        .get_top_searches(&params.locale, &period, params.limit)
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
            Json(ApiResponse::err(e.to_string()))
        }
    }
}
