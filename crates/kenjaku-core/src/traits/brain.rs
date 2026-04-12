use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::types::intent::IntentClassification;
use crate::types::locale::Locale;
use crate::types::message::Message;
use crate::types::search::{LlmResponse, RetrievedChunk, StreamChunk, TranslationResult};

/// LLM-agnostic facade for all model interactions in the search pipeline.
///
/// The Brain trait abstracts prompt construction, conversation assembly,
/// and LLM invocation behind a single interface. Implementations own the
/// prompt templates and conversation-to-message mapping, so the orchestrator
/// never touches provider-specific types.
///
/// The `SearchOrchestrator` depends on `Arc<dyn Brain>` and routes all
/// intent classification, translation, generation, and suggestion calls
/// through this trait.
#[async_trait]
pub trait Brain: Send + Sync {
    /// Classify the intent of a user query.
    async fn classify_intent(
        &self,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<IntentClassification>;

    /// Normalize a query into canonical English AND detect its source
    /// locale in a single LLM call.
    async fn translate(&self, query: &str, cancel: &CancellationToken)
    -> Result<TranslationResult>;

    /// Generate a complete (non-streaming) LLM response.
    ///
    /// `messages` is the fully-assembled conversation (system instruction,
    /// history turns, current user turn with context). Built by the
    /// `ConversationAssembler`.
    ///
    /// `chunks` are the retrieved context chunks used to build the prompt.
    /// Passed separately so the Brain can attach provider-specific tools
    /// (e.g. Gemini's `google_search`) based on whether chunks are present.
    ///
    /// `locale` is the detected answer locale, used to configure
    /// provider-specific features (e.g. Gemini's `google_search` tool).
    async fn generate(
        &self,
        messages: &[Message],
        chunks: &[RetrievedChunk],
        locale: Locale,
        cancel: &CancellationToken,
    ) -> Result<LlmResponse>;

    /// Generate a streaming LLM response. Same semantics as `generate`
    /// but returns a token stream.
    async fn generate_stream(
        &self,
        messages: &[Message],
        chunks: &[RetrievedChunk],
        locale: Locale,
        cancel: &CancellationToken,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>>;

    /// Generate follow-up query suggestions based on a query and answer.
    async fn suggest(
        &self,
        query: &str,
        answer: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<String>>;
}
