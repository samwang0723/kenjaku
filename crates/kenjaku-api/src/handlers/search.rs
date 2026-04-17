use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use futures::stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info};

use crate::AppState;
use crate::dto::request::SearchRequestDto;
use crate::dto::response::{ApiResponse, SearchResponseDto};
use crate::extractors::TenantCtx;

use kenjaku_core::types::search::SearchRequest;
use kenjaku_core::types::tenant::TenantContext;
use kenjaku_service::search::SearchStreamOutput;

/// Maximum query length in characters.
const MAX_QUERY_LENGTH: usize = 2000;
/// Maximum top_k value.
const MAX_TOP_K: usize = 100;
/// Cap header-supplied identifiers (session_id / request_id) at this length
/// to avoid log/cache-key amplification via pathological clients.
const MAX_ID_LEN: usize = 128;

/// Look up a header by name, trim, and enforce [`MAX_ID_LEN`].
pub(crate) fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= MAX_ID_LEN)
        .map(str::to_owned)
}

/// POST /api/v1/search
pub async fn search(
    TenantCtx(tctx): TenantCtx,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(dto): Json<SearchRequestDto>,
) -> impl IntoResponse {
    // Validate query length
    if dto.query.trim().is_empty() {
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

    // Session + request ids:
    // - Header wins, then body field, then a fresh server-generated UUID.
    // - request_id is ALWAYS server-generated when absent.
    // - session_id is also generated server-side on first request of a
    //   page session; the client captures it from the response and sends
    //   it back as `X-Session-Id` on subsequent queries.
    // The same `device_session_id` is reused inside SearchService for
    // LocaleMemory and SessionHistoryStore keying.
    let header_session_id = header_str(&headers, "x-session-id");
    let session_id = header_session_id
        .clone()
        .or(dto.session_id)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let request_id = header_str(&headers, "x-request-id")
        .or(dto.request_id)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // SearchService uses the resolved session id for locale memory and
    // history keying. We pass an Option here so a missing header doesn't
    // accidentally write under a fresh-uuid key (the LocaleMemory pipeline
    // tolerates None and uses the body session_id in that case).
    let device_session_id = Some(session_id.clone());

    let req = SearchRequest {
        query: dto.query,
        session_id,
        request_id,
        streaming: dto.streaming,
        top_k,
    };

    // Phase 3e: tctx arrives via the `TenantCtx` extractor, populated
    // by the auth middleware (always on — every request is JWT-validated).

    if req.streaming {
        return search_streaming(state, req, tctx, device_session_id)
            .await
            .into_response();
    }

    match state
        .search_service
        .search(&req, &tctx, device_session_id.as_deref())
        .await
    {
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

/// SSE streaming search. Emits three named events:
///
/// - `event: start` with `StreamStartMetadata` — sent once before the first
///   token arrives (intent, translated_query, retrieval_count, etc.).
/// - `event: delta` with `{"text": "..."}` — one per token delta from the LLM.
/// - `event: done` with `StreamDoneMetadata` — sent after the last delta
///   (total latency, sources, suggestions, model).
/// - `event: error` with `{"error": "..."}` — on any failure.
async fn search_streaming(
    state: Arc<AppState>,
    req: SearchRequest,
    tctx: TenantContext,
    device_session_id: Option<String>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    info!(request_id = %req.request_id, "SSE streaming handler started");
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(100);
    let request_id = req.request_id.clone();

    tokio::spawn(async move {
        // Open the stream — this runs intent classify + translate + retrieve.
        let out = match state
            .search_service
            .search_stream(&req, &tctx, device_session_id.as_deref())
            .await
        {
            Ok(out) => out,
            Err(e) => {
                error!(request_id = %request_id, error = %e, "SSE search_stream failed");
                let _ = tx
                    .send(Ok(Event::default().event("error").data(format!(
                        "{{\"error\":{}}}",
                        serde_json::to_string(e.user_message())
                            .unwrap_or_else(|_| "\"error\"".into())
                    ))))
                    .await;
                return;
            }
        };

        // Destructure so the stream and context can be used independently.
        let SearchStreamOutput {
            start_metadata,
            mut stream,
            context,
        } = out;

        // Emit the `start` event with all preamble metadata.
        let start_json = match serde_json::to_string(&start_metadata) {
            Ok(s) => s,
            Err(e) => {
                error!(error = %e, "Failed to serialize start metadata");
                return;
            }
        };
        if tx
            .send(Ok(Event::default().event("start").data(start_json)))
            .await
            .is_err()
        {
            return; // client disconnected
        }

        // Stream token deltas. We also harvest any google_search grounding
        // sources Gemini attaches to the final event(s) so the `done`
        // payload can show them alongside the internal chunk sources.
        // Latest-wins usage accounting: Gemini attaches `usageMetadata` on
        // the final SSE event, so we overwrite on every chunk and keep
        // the last populated value for the generator's `LlmCall`.
        let generator_started = Instant::now();
        let mut accumulated = String::new();
        let mut grounding_sources: Vec<kenjaku_core::types::search::LlmSource> = Vec::new();
        let mut last_usage: Option<kenjaku_core::types::search::LlmUsage> = None;
        let mut stream_error: Option<kenjaku_core::error::Error> = None;
        let mut accumulated_suggestions: Vec<String> = Vec::new();
        let mut accumulated_assets: Vec<kenjaku_core::types::assets::Asset> = Vec::new();
        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    if !chunk.delta.is_empty() {
                        accumulated.push_str(&chunk.delta);
                        let delta_json = serde_json::to_string(&serde_json::json!({
                            "text": chunk.delta,
                        }))
                        .unwrap_or_else(|_| "{\"text\":\"\"}".into());
                        if tx
                            .send(Ok(Event::default().event("delta").data(delta_json)))
                            .await
                            .is_err()
                        {
                            info!(request_id = %request_id, "SSE client disconnected mid-stream");
                            return;
                        }
                    }
                    if let Some(g) = chunk.grounding {
                        grounding_sources.extend(g);
                    }
                    if let Some(u) = chunk.usage {
                        last_usage = Some(u);
                    }
                    // Merged-JSON terminal chunk carries suggestions +
                    // assets parsed by the provider's streaming JSON
                    // parser. Accumulate here so `complete_stream`
                    // sees them without needing a second LLM call.
                    if let Some(s) = chunk.suggestions {
                        accumulated_suggestions = s;
                    }
                    if let Some(a) = chunk.assets {
                        accumulated_assets = a;
                    }
                    if chunk.finished {
                        break;
                    }
                }
                Err(e) => {
                    error!(request_id = %request_id, error = %e, "SSE chunk error");
                    stream_error = Some(e);
                    break;
                }
            }
        }
        let generator_latency_ms = generator_started.elapsed().as_millis() as u64;
        drop(stream);

        if let Some(e) = stream_error {
            let _ = tx
                .send(Ok(Event::default().event("error").data(format!(
                    "{{\"error\":{}}}",
                    serde_json::to_string(e.user_message()).unwrap_or_else(|_| "\"error\"".into())
                ))))
                .await;
            return;
        }

        // Build the streaming generator's accounting entry from the
        // last seen `usageMetadata` + the measured wall-clock latency.
        // `None` when the provider never attached usage, in which case
        // the generator call is simply omitted from `usage.calls`.
        let generator_call = last_usage.map(|u| kenjaku_core::types::usage::LlmCall {
            purpose: "generate_stream".to_string(),
            model: context.llm_model.clone(),
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            cost_usd: u.cost_usd.unwrap_or(0.0),
            latency_ms: generator_latency_ms,
        });

        // Compute final done metadata (suggestions + latency, plus
        // resolved+merged grounding sources) and persist conversation.
        let done_metadata = state
            .search_service
            .complete_stream(
                context,
                &accumulated,
                grounding_sources,
                generator_call,
                accumulated_suggestions,
                accumulated_assets,
            )
            .await;

        let done_json = serde_json::to_string(&done_metadata).unwrap_or_else(|_| "{}".into());
        let _ = tx
            .send(Ok(Event::default().event("done").data(done_json)))
            .await;

        info!(
            request_id = %request_id,
            latency_ms = done_metadata.latency_ms,
            "SSE stream completed"
        );
    });

    Sse::new(ReceiverStream::new(rx))
}
