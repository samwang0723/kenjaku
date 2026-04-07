use std::sync::Arc;

use tracing::instrument;

use kenjaku_core::error::Result;
use kenjaku_core::traits::llm::LlmProvider;

/// Service for translating non-English queries to English.
/// Source language is auto-detected by the LLM.
#[derive(Clone)]
pub struct TranslationService {
    llm: Arc<dyn LlmProvider>,
}

impl TranslationService {
    pub fn new(llm: Arc<dyn LlmProvider>) -> Self {
        Self { llm }
    }

    /// Translate a query to English. The source language is auto-detected.
    #[instrument(skip(self))]
    pub async fn translate(&self, query: &str) -> Result<String> {
        self.llm.translate(query, "en").await
    }
}
