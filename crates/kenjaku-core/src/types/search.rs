use std::pin::Pin;
use std::time::Instant;

use chrono::{DateTime, Utc};
use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::component::Component;
use super::intent::Intent;
use super::locale::{DetectedLocale, Locale};
use crate::error::Result;

/// Incoming search request from the API layer.
///
/// The locale is no longer carried on the request — it's detected by the
/// translator from the query text itself and surfaced in
/// `SearchMetadata.locale` / `StreamStartMetadata.locale`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub session_id: String,
    pub request_id: String,
    #[serde(default)]
    pub streaming: bool,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

fn default_top_k() -> usize {
    10
}

/// Output of the LLM-based translator/normalizer.
///
/// The translator runs on every query and returns both the canonical
/// English form (used for retrieval) AND the detected source locale
/// (used to drive the answer-language `systemInstruction`).
#[derive(Debug, Clone)]
pub struct TranslationResult {
    /// Canonical English form of the user's query, ready for retrieval.
    pub normalized: String,
    /// Source language the translator detected.
    pub detected_locale: DetectedLocale,
}

/// Provenance of the locale we ended up using for an answer. Surfaced
/// in `SearchMetadata` / `StreamStartMetadata` so the frontend can show
/// "detected: zh-TW (llm)" vs "(fallback)" and pick the right font/RTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectedLocaleSource {
    /// Translator returned a supported locale (the happy path).
    LlmDetected,
    /// Translator failed, returned an unsupported tag, or never ran —
    /// we fell back to English.
    FallbackEn,
}

/// Final search response returned to the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub request_id: String,
    pub session_id: String,
    pub components: Vec<Component>,
    pub metadata: SearchMetadata,
}

/// Metadata about the search execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMetadata {
    pub original_query: String,
    pub translated_query: Option<String>,
    /// Detected source locale (the translator's output, or `en` on fallback).
    pub locale: Locale,
    /// Provenance of the locale: `llm_detected` on the happy path,
    /// `fallback_en` if the translator failed or returned an unsupported tag.
    pub detected_locale_source: DetectedLocaleSource,
    pub intent: Intent,
    pub retrieval_count: usize,
    pub latency_ms: u64,
    /// Web tier provenance — tells the client whether and how the
    /// real-time web search backend (Brave / Serper / …) was hit on
    /// this request, and how many results it contributed.
    #[serde(default)]
    pub grounding: GroundingInfo,
}

/// What grounding tier(s) supplied chunks/sources for this request.
/// Skipped on the wire when both flags are false to keep the JSON tidy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroundingInfo {
    /// True iff `SearchService` invoked its configured `WebSearchProvider`
    /// (Brave / Serper / …) for this request and got back ≥1 result.
    #[serde(default)]
    pub web_search_used: bool,
    /// Identifier of the web provider that supplied results, if any
    /// (e.g. `"brave"`). `None` when `web_search_used == false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search_provider: Option<String>,
    /// How many synthetic chunks the web tier added.
    #[serde(default)]
    pub web_search_count: usize,
    /// True iff Gemini's built-in `google_search` tool fired and
    /// attached `groundingMetadata` to its response. Currently rare in
    /// practice (the preview model silently ignores the tool) but
    /// reported when present so the client knows it happened.
    #[serde(default)]
    pub gemini_grounding_used: bool,
}

/// A chunk retrieved from the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievedChunk {
    pub doc_id: String,
    pub chunk_id: String,
    pub title: String,
    pub original_content: String,
    pub contextualized_content: String,
    pub source_url: Option<String>,
    pub score: f32,
    pub retrieval_method: RetrievalMethod,
}

/// How a chunk was retrieved.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RetrievalMethod {
    Vector,
    FullText,
    Hybrid,
    /// Chunk originated from a live web search provider (Brave, Serper,
    /// etc.) and was injected into the chunk list as a synthetic
    /// `[Source N]` entry alongside internal corpus chunks.
    Web,
}

/// Response from the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub answer: String,
    pub sources: Vec<LlmSource>,
    pub model: String,
    pub usage: Option<LlmUsage>,
}

/// A source cited by the LLM (e.g., from google_search tool).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmSource {
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
}

/// Token usage from LLM call, with estimated cost based on model + tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    /// Estimated cost in USD for this call, factoring in the service tier
    /// multiplier. `None` if pricing data is unavailable for the model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// A single SSE stream chunk (text delta).
///
/// `grounding` carries Gemini google_search grounding sources that the
/// streaming pipeline observed on this event (typically populated only on
/// the final event with `finished = true`). Old consumers that ignore this
/// field continue to work; the API handler accumulates and forwards it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    pub delta: String,
    pub chunk_type: StreamChunkType,
    pub finished: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grounding: Option<Vec<LlmSource>>,
}

/// Metadata sent at the START of a streaming search — everything we know
/// before the LLM begins producing tokens. Allows the client to populate its
/// debug panel immediately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamStartMetadata {
    pub request_id: String,
    pub session_id: String,
    pub original_query: String,
    pub translated_query: Option<String>,
    /// Detected source locale (translator output, or `en` on fallback).
    pub locale: super::locale::Locale,
    /// Provenance of the locale: `llm_detected` on the happy path,
    /// `fallback_en` if the translator failed or returned an unsupported tag.
    pub detected_locale_source: DetectedLocaleSource,
    pub intent: super::intent::Intent,
    pub retrieval_count: usize,
    pub preamble_latency_ms: u64,
    /// Web tier provenance — known by the time we emit `start` since
    /// the web search runs in the preamble before the LLM stream opens.
    #[serde(default)]
    pub grounding: GroundingInfo,
}

/// Metadata sent at the END of a streaming search — total latency and
/// auxiliary components (sources, suggestions) that weren't part of the
/// token stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamDoneMetadata {
    pub latency_ms: u64,
    pub sources: Vec<LlmSource>,
    pub suggestions: Vec<String>,
    pub llm_model: String,
    /// Mirrors `start.grounding`, refreshed with anything we learned
    /// during the stream (e.g. Gemini grounding metadata that arrived
    /// in the final event).
    #[serde(default)]
    pub grounding: GroundingInfo,
}

/// Type of content in a stream chunk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StreamChunkType {
    Answer,
    Source,
    Suggestion,
    Metadata,
    Error,
}

/// Document metadata for ingestion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChunk {
    pub doc_id: String,
    pub chunk_id: String,
    pub title: String,
    pub original_content: String,
    pub contextualized_content: String,
    pub source_url: Option<String>,
    pub doc_type: DocumentType,
    pub ingested_at: DateTime<Utc>,
    pub embedding: Option<Vec<f32>>,
}

/// Supported document types for ingestion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DocumentType {
    Markdown,
    PlainText,
    Docx,
    Html,
}

impl std::fmt::Display for DocumentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Markdown => write!(f, "markdown"),
            Self::PlainText => write!(f, "plaintext"),
            Self::Docx => write!(f, "docx"),
            Self::Html => write!(f, "html"),
        }
    }
}

/// Output of a streaming search.
///
/// The API handler drains `stream`, accumulating token deltas and any
/// grounding sources, then hands the `context` plus the accumulated
/// answer to the pipeline's `complete_stream` method to produce the
/// final `done` metadata.
pub struct SearchStreamOutput {
    pub start_metadata: StreamStartMetadata,
    pub stream: Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>,
    pub context: StreamContext,
}

/// RAII guard that cancels its [`CancellationToken`] on drop. Stored
/// inside [`StreamContext`] so that if the SSE connection is dropped
/// early, in-flight LLM / tool work is cancelled automatically.
pub struct CancelGuard(CancellationToken);

impl CancelGuard {
    pub fn new(token: CancellationToken) -> Self {
        Self(token)
    }

    /// Access the underlying token (e.g. to pass to tools).
    pub fn token(&self) -> &CancellationToken {
        &self.0
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Bookkeeping passed from a pipeline's `search_stream` to its
/// `complete_stream` — kept separate from the `Stream` so the handler
/// can move it into the completion call after dropping the stream.
pub struct StreamContext {
    pub sources: Vec<LlmSource>,
    pub llm_model: String,
    pub start_instant: Instant,
    /// Web tier provenance captured during the preamble (before the
    /// LLM stream opens). `complete_stream` may also flip
    /// `gemini_grounding_used` if Gemini attached grounding metadata
    /// to the stream.
    pub grounding: GroundingInfo,
    pub request_id: String,
    pub session_id: String,
    /// Stable device/session key used for in-memory history and locale
    /// memory. Prefer `X-Session-Id` header; falls back to body session_id.
    pub history_key: String,
    pub query: String,
    pub locale: Locale,
    pub intent: Intent,
    /// Cancellation guard that fires on drop, ensuring in-flight work
    /// is cancelled when the SSE connection disconnects.
    pub _cancel_guard: CancelGuard,
}
