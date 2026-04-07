use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::component::Component;
use super::intent::Intent;
use super::locale::Locale;

/// Incoming search request from the API layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub locale: Locale,
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
    pub locale: Locale,
    pub intent: Intent,
    pub retrieval_count: usize,
    pub latency_ms: u64,
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

/// Token usage from LLM call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// A single SSE stream chunk (text delta).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    pub delta: String,
    pub chunk_type: StreamChunkType,
    pub finished: bool,
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
    pub locale: super::locale::Locale,
    pub intent: super::intent::Intent,
    pub retrieval_count: usize,
    pub preamble_latency_ms: u64,
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
