use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::types::intent::{Intent, IntentClassification};
use crate::types::locale::{DetectedLocale, Locale};
use crate::types::message::Message;
use crate::types::preprocess::QueryPreprocessing;
use crate::types::search::{LlmResponse, RetrievedChunk, StreamChunk, TranslationResult};
use crate::types::usage::LlmCall;

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
///
/// **Phase 2 note:** `Brain` is still the orchestrator-facing contract,
/// but the concrete implementation wired into the pipeline is now
/// `CompositeBrain` (in `kenjaku-service`) which composes three
/// sub-traits (`Classifier`, `Translator`, `Generator`). The default
/// methods [`Brain::has_web_grounding`] and [`Brain::model_name`]
/// replace two Phase 1 leaks — the hardcoded `has_web_grounding: bool`
/// constructor arg on `SinglePassPipeline` and the hardcoded `"gemini"`
/// model name in streaming `done` metadata.
#[async_trait]
pub trait Brain: Send + Sync {
    /// Classify the intent of a user query.
    ///
    /// Returns the classification paired with an optional [`LlmCall`]
    /// accounting entry so the pipeline can surface per-call token
    /// usage + cost on `SearchMetadata.usage`. `None` when the
    /// underlying provider cannot report usage for this call.
    async fn classify_intent(
        &self,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<(IntentClassification, Option<LlmCall>)>;

    /// Normalize a query into canonical English AND detect its source
    /// locale in a single LLM call.
    ///
    /// Returns the translation result paired with an optional
    /// [`LlmCall`] accounting entry.
    async fn translate(
        &self,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<(TranslationResult, Option<LlmCall>)>;

    /// **Merged preamble** (used when `pipeline.mode = two_call`):
    /// produce intent + translation + locale-detection in one shot.
    ///
    /// Returns the unified preprocessing result alongside the
    /// [`LlmCall`] accounting entries from however many calls the
    /// concrete impl actually made — one in `two_call` mode, two in
    /// `single_pass` mode.
    ///
    /// Default impl preserves today's pipeline behavior: runs
    /// `classify_intent` and `translate` in parallel via `futures::join!`
    /// and assembles the unified result. `CompositeBrain` overrides
    /// this to call `LlmProvider::preprocess_query` when configured
    /// for `two_call`.
    ///
    /// **Graceful degradation:** an error in either sub-call is
    /// absorbed — the failing side defaults to (Unknown intent) or
    /// (raw query, en locale) respectively, and the surviving side's
    /// `LlmCall` is still recorded. This matches the pre-merge
    /// pipeline behavior where translator failures fell through to
    /// `resolve_translation`'s en fallback while the classifier's
    /// usage was kept. Returning `Err` here would silently drop
    /// observability data the operator wants to see.
    async fn preprocess(
        &self,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<(QueryPreprocessing, Vec<LlmCall>)> {
        let (intent_result, translate_result) = futures::join!(
            self.classify_intent(query, cancel),
            self.translate(query, cancel),
        );
        let mut calls = Vec::with_capacity(2);
        let intent = match intent_result {
            Ok((classification, call)) => {
                if let Some(c) = call {
                    calls.push(c);
                }
                classification
            }
            Err(e) => {
                tracing::warn!(error = %e, "classify_intent failed, defaulting to Unknown");
                IntentClassification {
                    intent: Intent::Unknown,
                    confidence: 0.0,
                }
            }
        };
        let translation = match translate_result {
            Ok((tr, call)) => {
                if let Some(c) = call {
                    calls.push(c);
                }
                tr
            }
            Err(e) => {
                tracing::warn!(error = %e, "translate failed, falling back to (raw, fallback_en)");
                // Emit `Unsupported { tag: "" }` so the downstream
                // `resolve_translation` records this as
                // `DetectedLocaleSource::FallbackEn` (matches the pre-merge
                // pipeline's provenance signal). `Supported(En)` would
                // incorrectly read as `LlmDetected`.
                TranslationResult {
                    normalized: query.to_string(),
                    detected_locale: DetectedLocale::Unsupported { tag: String::new() },
                }
            }
        };
        Ok((
            QueryPreprocessing {
                intent,
                translation,
            },
            calls,
        ))
    }

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

    /// Generate a streaming LLM response. Same semantics as `generate`
    /// but returns a token stream.
    ///
    /// Usage for streaming is attached to individual `StreamChunk`s
    /// (typically only the terminal chunk with `finished = true`);
    /// see [`StreamChunk::usage`].
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

    /// Whether the underlying Generator attaches its own built-in
    /// web-grounding tool (e.g. Gemini's `google_search`).
    ///
    /// The pipeline queries this before assembling the conversation so
    /// the `ConversationAssembler` can pick the right system-instruction
    /// variant. Default: `false`. `CompositeBrain` overrides this to
    /// forward to its `Generator`.
    fn has_web_grounding(&self) -> bool {
        false
    }

    /// Short identifier for the Generator's underlying model, echoed in
    /// streaming `done` metadata. Default: `"unknown"`. `CompositeBrain`
    /// overrides this to forward to its `Generator`.
    fn model_name(&self) -> &str {
        "unknown"
    }
}
