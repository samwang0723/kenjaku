use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use axum::Json;
use futures::stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info};

use crate::dto::request::SearchRequestDto;
use crate::dto::response::{ApiResponse, SearchResponseDto};
use crate::AppState;

use kenjaku_core::types::search::SearchRequest;

/// Maximum query length in characters.
const MAX_QUERY_LENGTH: usize = 2000;
/// Maximum top_k value.
const MAX_TOP_K: usize = 100;

/// POST /api/v1/search
pub async fn search(
    State(state): State<Arc<AppState>>,
    Json(dto): Json<SearchRequestDto>,
) -> impl IntoResponse {
    // Validate locale
    let locale = match dto.parse_locale() {
        Ok(l) => l,
        Err(e) => {
            return Json(ApiResponse::<SearchResponseDto>::err(
                e.user_message().to_string(),
            ))
            .into_response();
        }
    };

    // Validate query length
    if dto.query.is_empty() {
        return Json(ApiResponse::<SearchResponseDto>::err(
            "Query cannot be empty".to_string(),
        ))
        .into_response();
    }
    if dto.query.len() > MAX_QUERY_LENGTH {
        return Json(ApiResponse::<SearchResponseDto>::err(format!(
            "Query exceeds maximum length of {MAX_QUERY_LENGTH} characters"
        )))
        .into_response();
    }

    // Clamp top_k
    let top_k = dto.top_k.unwrap_or(10).min(MAX_TOP_K);

    let req = SearchRequest {
        query: dto.query,
        locale,
        session_id: dto.session_id,
        request_id: dto.request_id,
        streaming: dto.streaming,
        top_k,
    };

    if req.streaming {
        return search_streaming(state, req).await.into_response();
    }

    match state.search_service.search(&req).await {
        Ok(response) => {
            let dto: SearchResponseDto = response.into();
            Json(ApiResponse::ok(dto)).into_response()
        }
        Err(e) => {
            error!(error = %e, "Search failed");
            Json(ApiResponse::<SearchResponseDto>::err(
                e.user_message().to_string(),
            ))
            .into_response()
        }
    }
}

async fn search_streaming(
    state: Arc<AppState>,
    req: SearchRequest,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    info!(request_id = %req.request_id, "SSE streaming handler started");
    let (tx, rx) = tokio::sync::mpsc::channel(100);
    let request_id = req.request_id.clone();

    tokio::spawn(async move {
        info!(request_id = %request_id, "SSE spawned task running");
        match state.search_service.search_stream(&req).await {
            Ok(mut stream) => {
                info!(request_id = %request_id, "SSE got stream from search_stream, starting to read chunks");
                let mut chunk_count = 0u32;
                while let Some(chunk_result) = stream.next().await {
                    match chunk_result {
                        Ok(chunk) => {
                            chunk_count += 1;
                            let data = serde_json::to_string(&chunk).unwrap_or_default();
                            let event = Event::default().data(data);
                            if tx.send(Ok(event)).await.is_err() {
                                info!(request_id = %request_id, chunks = chunk_count, "SSE client disconnected");
                                break;
                            }
                            if chunk.finished {
                                info!(request_id = %request_id, chunks = chunk_count, "SSE stream finished");
                                break;
                            }
                        }
                        Err(e) => {
                            error!(request_id = %request_id, error = %e, "SSE chunk error");
                            let event = Event::default()
                                .event("error")
                                .data(e.user_message().to_string());
                            let _ = tx.send(Ok(event)).await;
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                error!(request_id = %request_id, error = %e, "SSE search_stream failed");
                let event = Event::default()
                    .event("error")
                    .data(e.user_message().to_string());
                let _ = tx.send(Ok(event)).await;
            }
        }
    });

    Sse::new(ReceiverStream::new(rx))
}
