use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::error::Result;
use crate::types::search::{LlmResponse, RetrievedChunk, StreamChunk};

/// Trait for LLM providers. Implementations can use Gemini, OpenAI, Claude, etc.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Generate a complete response given a query and retrieved context.
    async fn generate(
        &self,
        query: &str,
        context: &[RetrievedChunk],
    ) -> Result<LlmResponse>;

    /// Generate a streaming response.
    async fn generate_stream(
        &self,
        query: &str,
        context: &[RetrievedChunk],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>>;

    /// Translate text to the target locale. The source language is auto-detected
    /// by the implementation (callers should not pass a `from_locale`).
    async fn translate(&self, text: &str, to_locale: &str) -> Result<String>;

    /// Generate follow-up query suggestions based on the query and answer.
    async fn suggest(
        &self,
        query: &str,
        answer: &str,
    ) -> Result<Vec<String>>;
}

/// Trait for chunk contextualization (typically using Claude).
#[async_trait]
pub trait Contextualizer: Send + Sync {
    /// Generate a short context description for a chunk within its parent document.
    async fn contextualize(
        &self,
        document_content: &str,
        chunk_content: &str,
    ) -> Result<String>;
}
