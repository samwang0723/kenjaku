use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use axum::Json;
use futures::stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;

use crate::dto::request::SearchRequestDto;
use crate::dto::response::{ApiResponse, SearchResponseDto};
use crate::AppState;

use kenjaku_core::types::search::SearchRequest;

/// POST /api/v1/search
pub async fn search(
    State(state): State<Arc<AppState>>,
    Json(dto): Json<SearchRequestDto>,
) -> impl IntoResponse {
    // Validate locale
    let locale = match dto.parse_locale() {
        Ok(l) => l,
        Err(e) => {
            return Json(ApiResponse::<SearchResponseDto>::err(e.to_string())).into_response();
        }
    };

    let req = SearchRequest {
        query: dto.query,
        locale,
        session_id: dto.session_id,
        request_id: dto.request_id,
        streaming: dto.streaming,
        top_k: dto.top_k.unwrap_or(10),
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
            Json(ApiResponse::<SearchResponseDto>::err(e.to_string())).into_response()
        }
    }
}

async fn search_streaming(
    state: Arc<AppState>,
    req: SearchRequest,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel(100);

    tokio::spawn(async move {
        match state.search_service.search_stream(&req).await {
            Ok(mut stream) => {
                while let Some(chunk_result) = stream.next().await {
                    match chunk_result {
                        Ok(chunk) => {
                            let data = serde_json::to_string(&chunk).unwrap_or_default();
                            let event = Event::default().data(data);
                            if tx.send(Ok(event)).await.is_err() {
                                break;
                            }
                            if chunk.finished {
                                break;
                            }
                        }
                        Err(e) => {
                            let event = Event::default()
                                .event("error")
                                .data(e.to_string());
                            let _ = tx.send(Ok(event)).await;
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                let event = Event::default().event("error").data(e.to_string());
                let _ = tx.send(Ok(event)).await;
            }
        }
    });

    Sse::new(ReceiverStream::new(rx))
}
