use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::error::{Error, Result};
use crate::types::message::Message;
use crate::types::preprocess::QueryPreprocessing;
use crate::types::search::{LlmResponse, LlmUsage, StreamChunk, TranslationResult};
use crate::types::suggestion::ClusterQuestions;

/// Trait for LLM providers. Implementations can use Gemini, OpenAI, Claude, etc.
///
/// The two main generation methods (`generate` and `generate_stream`) accept
/// a pre-assembled `&[Message]` conversation. The caller (typically `Brain`
/// via `ConversationAssembler`) is responsible for building the system
/// instruction, injecting retrieved-context chunks, and interleaving
/// conversation history. The provider maps the LLM-agnostic `Message`
/// sequence to its native wire format internally (e.g. `messages_to_wire`
/// for Gemini).
///
/// Utility methods (`generate_brief`, `translate`, `suggest`,
/// `generate_cluster_questions`) remain single-shot convenience endpoints
/// that build their own prompts internally.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Generate a complete response from a pre-assembled message sequence.
    ///
    /// The `messages` slice follows the convention:
    /// - `messages[0]` with `Role::System` — the system instruction
    /// - Alternating `Role::User` / `Role::Assistant` — conversation history
    /// - Final `Role::User` — the current turn (query + retrieved context)
    ///
    /// Implementations map this to their native wire format and attach
    /// provider-specific tools (e.g. Gemini's `google_search`) based on
    /// their own configuration.
    async fn generate(&self, messages: &[Message]) -> Result<LlmResponse>;

    /// Generate a streaming response from a pre-assembled message sequence.
    /// See `generate` for the `messages` convention.
    async fn generate_stream(
        &self,
        messages: &[Message],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>>;

    /// Fast, stateless, tool-less completion — used for intent
    /// classification and other single-shot utility calls where the
    /// response is short, English, and doesn't need grounding. Skipping
    /// the google_search tool drops latency by several seconds.
    ///
    /// Default impl wraps the prompt in a single user message and calls
    /// `generate`. Real providers (Gemini) should override to cap
    /// max_tokens and skip the grounding tool entirely.
    async fn generate_brief(&self, prompt: &str) -> Result<LlmResponse> {
        self.generate(&[Message::user_text(prompt)]).await
    }

    /// Normalize a query into canonical English AND detect its source
    /// locale in a single LLM call. Source language is auto-detected;
    /// the implementation MUST NOT take a target locale (always English).
    ///
    /// Returns the translation result paired with optional usage
    /// accounting (token counts + cost) so the pipeline can surface
    /// per-call cost in the API response. `None` when the provider
    /// cannot report usage for this call.
    async fn translate(&self, text: &str) -> Result<(TranslationResult, Option<LlmUsage>)>;

    /// **Two-call pipeline (Phase A)** — merged preamble: classify intent
    /// + translate/normalize + detect locale in a single LLM call.
    ///
    /// Replaces the parallel `classify_intent` + `translate` pair when
    /// `pipeline.preamble_mode = merged_preamble` is set. Implementations should issue
    /// exactly one provider call and return the unified preprocessing
    /// result.
    ///
    /// Default impl returns [`Error::Internal`] so providers that don't
    /// support structured-output preprocessing (e.g. Claude
    /// contextualizer, OpenAI embedding-only providers) compile without
    /// rewrites. The pipeline gracefully falls back to the parallel
    /// path when this returns `Err`.
    async fn preprocess_query(
        &self,
        _query: &str,
    ) -> Result<(QueryPreprocessing, Option<LlmUsage>)> {
        Err(Error::Internal(
            "preprocess_query not implemented for this LlmProvider".to_string(),
        ))
    }

    /// Generate follow-up query suggestions based on the query and answer.
    ///
    /// Returns the suggestions paired with optional usage accounting
    /// so the pipeline can roll up per-call cost into
    /// `SearchMetadata.usage`. `None` when the provider cannot report
    /// usage for this call.
    async fn suggest(&self, query: &str, answer: &str) -> Result<(Vec<String>, Option<LlmUsage>)>;

    /// Generate a topic label + 3-5 native-phrased questions for every
    /// supported locale, given a representative excerpt of clustered
    /// document content. Used by `SuggestionRefreshWorker` — one call
    /// per cluster, all 8 locales in one response.
    ///
    /// Default impl returns `Error::Internal("not implemented")` so
    /// providers that don't supply default suggestions (e.g. Claude
    /// contextualizer) compile without rewrites.
    async fn generate_cluster_questions(&self, _excerpt: &str) -> Result<ClusterQuestions> {
        Err(Error::Internal(
            "generate_cluster_questions not implemented for this LlmProvider".to_string(),
        ))
    }
}

/// Trait for chunk contextualization (typically using Claude).
#[async_trait]
pub trait Contextualizer: Send + Sync {
    /// Generate a short context description for a chunk within its parent document.
    async fn contextualize(&self, document_content: &str, chunk_content: &str) -> Result<String>;
}
