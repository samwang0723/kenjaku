use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use tracing::error;

use crate::AppState;
use crate::dto::request::FeedbackRequestDto;
use crate::dto::response::{ApiResponse, FeedbackResponseDto};

use kenjaku_core::types::feedback::{CreateFeedbackRequest, FeedbackAction};

/// POST /api/v1/feedback
pub async fn create_feedback(
    State(state): State<Arc<AppState>>,
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

    let req = CreateFeedbackRequest {
        session_id: dto.session_id,
        request_id: dto.request_id,
        action,
        reason_category_id: dto.reason_category_id,
        description: dto.description,
    };

    match state.feedback_service.create(&req).await {
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
