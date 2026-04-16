use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::types::locale::Locale;
use crate::types::message::Message;
use crate::types::search::{LlmResponse, RetrievedChunk, StreamChunk};
use crate::types::usage::LlmCall;

/// Generator sub-trait extracted from the `Brain` god-trait as part of
/// the Phase 2 flexibility refactor.
///
/// A `Generator` owns the final answer-producing LLM call plus related
/// follow-up capabilities (streaming variant, suggestion generation).
/// It also exposes two small capability queries the pipeline needs
/// before it invokes the Generator:
///
/// - [`has_web_grounding`] — whether this Generator attaches its own
///   built-in web-search/grounding tool (Gemini's `google_search`). The
///   `ConversationAssembler` uses this to pick the right system
///   instruction variant.
/// - [`model_name`] — a short identifier the streaming path echoes back
///   in `StreamDoneMetadata.llm_model`. Replaces the hardcoded
///   `"gemini"` literal tracked in Phase 1 as a known leak.
///
/// Phase 2 keeps a single `GeminiBrain` instance serving as the
/// `Generator`; Phase 3 can swap in a different provider without
/// touching the pipeline or the `Brain` facade.
///
/// See `docs/architecture/flexibility-refactor-tech-spec.md` §3.3.3.
#[async_trait]
pub trait Generator: Send + Sync {
    /// Generate a complete (non-streaming) LLM response.
    ///
    /// Returns the LLM response paired with an optional [`LlmCall`]
    /// accounting entry.
    async fn generate(
        &self,
        messages: &[Message],
        chunks: &[RetrievedChunk],
        locale: Locale,
        cancel: &CancellationToken,
    ) -> Result<(LlmResponse, Option<LlmCall>)>;

    /// Generate a streaming LLM response. Same inputs as `generate`;
    /// returns a token stream. Usage is attached to `StreamChunk`s
    /// rather than returned here since it arrives on the final event.
    async fn generate_stream(
        &self,
        messages: &[Message],
        chunks: &[RetrievedChunk],
        locale: Locale,
        cancel: &CancellationToken,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>>;

    /// Generate follow-up query suggestions based on a query and answer.
    ///
    /// Returns the suggestions paired with an optional [`LlmCall`]
    /// accounting entry.
    async fn suggest(
        &self,
        query: &str,
        answer: &str,
        cancel: &CancellationToken,
    ) -> Result<(Vec<String>, Option<LlmCall>)>;

    /// Whether this Generator attaches its own built-in web-grounding
    /// tool (e.g. Gemini's `google_search`). Default: `false`.
    fn has_web_grounding(&self) -> bool {
        false
    }

    /// Short identifier for this Generator's underlying model,
    /// surfaced in streaming `done` metadata. Default: `"unknown"`.
    fn model_name(&self) -> &str {
        "unknown"
    }
}
