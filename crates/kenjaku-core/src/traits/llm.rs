use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::error::Result;
use crate::types::locale::Locale;
use crate::types::search::{LlmResponse, RetrievedChunk, StreamChunk, TranslationResult};

/// Trait for LLM providers. Implementations can use Gemini, OpenAI, Claude, etc.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Generate a complete response given a query and retrieved context.
    ///
    /// `answer_locale` is the locale the model must respond in — used to
    /// build the per-request `systemInstruction` so the answer is rendered
    /// in the user's source language regardless of the context language.
    async fn generate(
        &self,
        query: &str,
        context: &[RetrievedChunk],
        answer_locale: Locale,
    ) -> Result<LlmResponse>;

    /// Generate a streaming response. See `generate` for `answer_locale`.
    async fn generate_stream(
        &self,
        query: &str,
        context: &[RetrievedChunk],
        answer_locale: Locale,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>>;

    /// Normalize a query into canonical English AND detect its source
    /// locale in a single LLM call. Source language is auto-detected;
    /// the implementation MUST NOT take a target locale (always English).
    async fn translate(&self, text: &str) -> Result<TranslationResult>;

    /// Generate follow-up query suggestions based on the query and answer.
    async fn suggest(&self, query: &str, answer: &str) -> Result<Vec<String>>;
}

/// Trait for chunk contextualization (typically using Claude).
#[async_trait]
pub trait Contextualizer: Send + Sync {
    /// Generate a short context description for a chunk within its parent document.
    async fn contextualize(&self, document_content: &str, chunk_content: &str) -> Result<String>;
}
