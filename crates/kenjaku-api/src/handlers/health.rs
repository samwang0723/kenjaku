use std::sync::Arc;

use axum::extract::State;
use axum::Json;

use crate::dto::response::{ApiResponse, HealthResponseDto, ReadyCheckDto, ReadyResponseDto};
use crate::AppState;

/// GET /health
pub async fn health() -> Json<ApiResponse<HealthResponseDto>> {
    Json(ApiResponse::ok(HealthResponseDto {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }))
}

/// GET /ready
pub async fn ready(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<ReadyResponseDto>> {
    let mut checks = Vec::new();
    let mut all_ok = true;

    // Check Qdrant
    match state.qdrant.health_check().await {
        Ok(()) => checks.push(ReadyCheckDto {
            name: "qdrant".to_string(),
            status: "ok".to_string(),
            error: None,
        }),
        Err(_) => {
            all_ok = false;
            checks.push(ReadyCheckDto {
                name: "qdrant".to_string(),
                status: "error".to_string(),
                error: Some("unavailable".to_string()),
            });
        }
    }

    // Check PostgreSQL
    match kenjaku_infra::postgres::health_check(&state.pg_pool).await {
        Ok(()) => checks.push(ReadyCheckDto {
            name: "postgres".to_string(),
            status: "ok".to_string(),
            error: None,
        }),
        Err(_) => {
            all_ok = false;
            checks.push(ReadyCheckDto {
                name: "postgres".to_string(),
                status: "error".to_string(),
                error: Some("unavailable".to_string()),
            });
        }
    }

    // Check Redis
    match state.redis.health_check().await {
        Ok(()) => checks.push(ReadyCheckDto {
            name: "redis".to_string(),
            status: "ok".to_string(),
            error: None,
        }),
        Err(_) => {
            all_ok = false;
            checks.push(ReadyCheckDto {
                name: "redis".to_string(),
                status: "error".to_string(),
                error: Some("unavailable".to_string()),
            });
        }
    }

    let status = if all_ok { "ready" } else { "degraded" }.to_string();

    Json(ApiResponse::ok(ReadyResponseDto { status, checks }))
}
