//! `GeminiBrain` — the default `Brain` implementation wrapping an
//! `Arc<dyn LlmProvider>`.
//!
//! Phase 2 note: `GeminiBrain` now also implements the three sub-traits
//! (`Classifier`, `Translator`, `Generator`) so the same instance can
//! serve as all three capabilities when composed inside
//! `CompositeBrain`. The existing `Brain` impl stays so any caller
//! holding `Arc<dyn Brain>` keeps compiling.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use futures::Stream;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use kenjaku_core::error::Result;
use kenjaku_core::traits::brain::Brain;
use kenjaku_core::traits::classifier::Classifier;
use kenjaku_core::traits::generator::Generator;
use kenjaku_core::traits::intent::IntentClassifier;
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::traits::translator::Translator;
use kenjaku_core::types::intent::IntentClassification;
use kenjaku_core::types::locale::Locale;
use kenjaku_core::types::message::Message;
use kenjaku_core::types::search::{
    LlmResponse, LlmUsage, RetrievedChunk, StreamChunk, TranslationResult,
};
use kenjaku_core::types::usage::LlmCall;

/// Default `Brain` implementation backed by a single `LlmProvider`.
///
/// Owns no prompt templates — those live in `brain::prompt`. The Brain
/// composes messages via `ConversationAssembler` and delegates all LLM
/// I/O to the injected provider.
///
/// Phase 2: also implements `Classifier`, `Translator`, and `Generator`
/// so the same instance can serve all three roles under
/// `CompositeBrain`. The `has_web_grounding` and `model_name` fields
/// are set at construction from config (they're properties of the
/// underlying `GeminiProvider`, not inferable from the `LlmProvider`
/// trait itself).
pub struct GeminiBrain {
    llm: Arc<dyn LlmProvider>,
    intent_classifier: Arc<dyn IntentClassifier>,
    has_web_grounding: bool,
    model_name: String,
}

impl GeminiBrain {
    /// Constructs a `GeminiBrain`.
    ///
    /// - `has_web_grounding` — whether the underlying provider attaches
    ///   its own built-in web-grounding tool (Gemini's `google_search`).
    ///   Derived at DI time (currently `!config.web_search.enabled`).
    /// - `model_name` — short identifier echoed in streaming `done`
    ///   metadata. Usually the configured Gemini model name.
    pub fn new(
        llm: Arc<dyn LlmProvider>,
        intent_classifier: Arc<dyn IntentClassifier>,
        has_web_grounding: bool,
        model_name: String,
    ) -> Self {
        Self {
            llm,
            intent_classifier,
            has_web_grounding,
            model_name,
        }
    }
}

// ---- Brain impl (preserved for backwards-compat with any direct
// ---- Arc<dyn Brain> consumer) --------------------------------------------

/// Build an `LlmCall` from optional usage + provider-reported model +
/// measured latency. Returns `None` when the provider didn't attach
/// usage metadata so the aggregator can skip this call.
fn build_call(
    purpose: &str,
    model: String,
    usage: Option<&LlmUsage>,
    latency_ms: u64,
) -> Option<LlmCall> {
    usage.map(|u| LlmCall {
        purpose: purpose.to_string(),
        model,
        input_tokens: u.prompt_tokens,
        output_tokens: u.completion_tokens,
        cost_usd: u.cost_usd.unwrap_or(0.0),
        latency_ms,
    })
}

#[async_trait]
impl Brain for GeminiBrain {
    #[instrument(skip(self, _cancel), fields(query = %query))]
    async fn classify_intent(
        &self,
        query: &str,
        _cancel: &CancellationToken,
    ) -> Result<(IntentClassification, Option<LlmCall>)> {
        self.intent_classifier.classify(query).await
    }

    #[instrument(skip(self, _cancel))]
    async fn translate(
        &self,
        query: &str,
        _cancel: &CancellationToken,
    ) -> Result<(TranslationResult, Option<LlmCall>)> {
        let started = Instant::now();
        let (result, usage) = self.llm.translate(query).await?;
        let latency_ms = started.elapsed().as_millis() as u64;
        let call = build_call(
            "translate",
            self.model_name.clone(),
            usage.as_ref(),
            latency_ms,
        );
        Ok((result, call))
    }

    #[instrument(skip(self, messages, _chunks, _cancel), fields(locale = %locale, msg_count = messages.len()))]
    async fn generate(
        &self,
        messages: &[Message],
        _chunks: &[RetrievedChunk],
        locale: Locale,
        _cancel: &CancellationToken,
    ) -> Result<(LlmResponse, Option<LlmCall>)> {
        let _ = locale; // locale is baked into the system instruction in messages
        let started = Instant::now();
        let response = self.llm.generate(messages).await?;
        let latency_ms = started.elapsed().as_millis() as u64;
        let call = build_call(
            "generate",
            response.model.clone(),
            response.usage.as_ref(),
            latency_ms,
        );
        Ok((response, call))
    }

    #[instrument(skip(self, messages, _chunks, _cancel), fields(locale = %locale, msg_count = messages.len()))]
    async fn generate_stream(
        &self,
        messages: &[Message],
        _chunks: &[RetrievedChunk],
        locale: Locale,
        _cancel: &CancellationToken,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        let _ = locale; // locale is baked into the system instruction in messages
        self.llm.generate_stream(messages).await
    }

    fn has_web_grounding(&self) -> bool {
        self.has_web_grounding
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}

// ---- Sub-trait impls (Phase 2) ------------------------------------------

#[async_trait]
impl Classifier for GeminiBrain {
    async fn classify(
        &self,
        query: &str,
        _cancel: &CancellationToken,
    ) -> Result<(IntentClassification, Option<LlmCall>)> {
        self.intent_classifier.classify(query).await
    }
}

#[async_trait]
impl Translator for GeminiBrain {
    async fn translate(
        &self,
        query: &str,
        _cancel: &CancellationToken,
    ) -> Result<(TranslationResult, Option<LlmCall>)> {
        let started = Instant::now();
        let (result, usage) = self.llm.translate(query).await?;
        let latency_ms = started.elapsed().as_millis() as u64;
        let call = build_call(
            "translate",
            self.model_name.clone(),
            usage.as_ref(),
            latency_ms,
        );
        Ok((result, call))
    }
}

#[async_trait]
impl Generator for GeminiBrain {
    async fn generate(
        &self,
        messages: &[Message],
        _chunks: &[RetrievedChunk],
        _locale: Locale,
        _cancel: &CancellationToken,
    ) -> Result<(LlmResponse, Option<LlmCall>)> {
        let started = Instant::now();
        let response = self.llm.generate(messages).await?;
        let latency_ms = started.elapsed().as_millis() as u64;
        let call = build_call(
            "generate",
            response.model.clone(),
            response.usage.as_ref(),
            latency_ms,
        );
        Ok((response, call))
    }

    async fn generate_stream(
        &self,
        messages: &[Message],
        _chunks: &[RetrievedChunk],
        _locale: Locale,
        _cancel: &CancellationToken,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        self.llm.generate_stream(messages).await
    }

    fn has_web_grounding(&self) -> bool {
        self.has_web_grounding
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}
