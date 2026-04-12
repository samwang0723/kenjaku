//! `GeminiBrain` — the default `Brain` implementation wrapping an
//! `Arc<dyn LlmProvider>`.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use kenjaku_core::error::Result;
use kenjaku_core::traits::brain::Brain;
use kenjaku_core::traits::intent::IntentClassifier;
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::types::intent::IntentClassification;
use kenjaku_core::types::locale::Locale;
use kenjaku_core::types::message::Message;
use kenjaku_core::types::search::{LlmResponse, RetrievedChunk, StreamChunk, TranslationResult};

/// Default `Brain` implementation backed by a single `LlmProvider`.
///
/// Owns no prompt templates — those live in `brain::prompt`. The Brain
/// composes messages via `ConversationAssembler` and delegates all LLM
/// I/O to the injected provider.
pub struct GeminiBrain {
    llm: Arc<dyn LlmProvider>,
    intent_classifier: Arc<dyn IntentClassifier>,
}

impl GeminiBrain {
    pub fn new(llm: Arc<dyn LlmProvider>, intent_classifier: Arc<dyn IntentClassifier>) -> Self {
        Self {
            llm,
            intent_classifier,
        }
    }
}

#[async_trait]
impl Brain for GeminiBrain {
    #[instrument(skip(self, _cancel), fields(query = %query))]
    async fn classify_intent(
        &self,
        query: &str,
        _cancel: &CancellationToken,
    ) -> Result<IntentClassification> {
        self.intent_classifier.classify(query).await
    }

    #[instrument(skip(self, _cancel))]
    async fn translate(
        &self,
        query: &str,
        _cancel: &CancellationToken,
    ) -> Result<TranslationResult> {
        self.llm.translate(query).await
    }

    #[instrument(skip(self, messages, _chunks, _cancel), fields(locale = %locale, msg_count = messages.len()))]
    async fn generate(
        &self,
        messages: &[Message],
        _chunks: &[RetrievedChunk],
        locale: Locale,
        _cancel: &CancellationToken,
    ) -> Result<LlmResponse> {
        // The messages already contain the fully assembled conversation
        // (system instruction, history turns, current user turn with
        // context), built by the ConversationAssembler. Pass them
        // directly to the LlmProvider which maps to its native wire
        // format internally via messages_to_wire.
        let _ = locale; // locale is baked into the system instruction in messages
        self.llm.generate(messages).await
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

    #[instrument(skip(self, _cancel))]
    async fn suggest(
        &self,
        query: &str,
        answer: &str,
        _cancel: &CancellationToken,
    ) -> Result<Vec<String>> {
        self.llm.suggest(query, answer).await
    }
}
