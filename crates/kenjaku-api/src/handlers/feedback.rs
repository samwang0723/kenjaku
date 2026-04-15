use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use tracing::error;

use crate::AppState;
use crate::dto::request::FeedbackRequestDto;
use crate::dto::response::{ApiResponse, FeedbackResponseDto};
use crate::handlers::search::header_str;

use kenjaku_core::types::feedback::{CreateFeedbackRequest, FeedbackAction};
use kenjaku_core::types::tenant::TenantContext;

/// POST /api/v1/feedback
pub async fn create_feedback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(dto): Json<FeedbackRequestDto>,
) -> Json<ApiResponse<FeedbackResponseDto>> {
    let action: FeedbackAction = match dto.action.parse() {
        Ok(a) => a,
        Err(_) => {
            return Json(ApiResponse::err(format!(
                "Invalid action '{}'. Supported: like, dislike, cancel",
                dto.action
            )));
        }
    };

    // Prefer X-Session-Id / X-Request-Id headers; fall back to body.
    // Feedback must match an existing conversation, so we reject outright
    // if neither source supplied a request id — no silent UUID generation
    // here (unlike /search which can start a fresh conversation).
    let session_id = match header_str(&headers, "x-session-id").or(dto.session_id) {
        Some(s) => s,
        None => return Json(ApiResponse::err("session_id is required".to_string())),
    };
    let request_id = match header_str(&headers, "x-request-id").or(dto.request_id) {
        Some(r) => r,
        None => return Json(ApiResponse::err("request_id is required".to_string())),
    };

    let req = CreateFeedbackRequest {
        session_id,
        request_id,
        action,
        reason_category_id: dto.reason_category_id,
        description: dto.description,
    };

    // 3c: replace with TenantContext extractor driven by the JWT
    // middleware. Until then every request resolves to the `public`
    // tenant.
    let tctx = TenantContext::public();

    match state.feedback_service.create(&tctx, &req).await {
        Ok(feedback) => Json(ApiResponse::ok(FeedbackResponseDto {
            id: feedback.id.to_string(),
            session_id: feedback.session_id,
            request_id: feedback.request_id,
            action: feedback.action.to_string(),
            created_at: feedback.created_at.to_rfc3339(),
        })),
        Err(e) => {
            error!(error = %e, "Create feedback failed");
            Json(ApiResponse::err(e.user_message().to_string()))
        }
    }
}
