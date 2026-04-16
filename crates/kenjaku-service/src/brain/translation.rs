use std::sync::Arc;

use tracing::instrument;

use kenjaku_core::error::Result;
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::types::search::{LlmUsage, TranslationResult};

/// Service for normalizing user queries and detecting their source locale
/// in a single LLM call. Returns canonical English (used for retrieval)
/// AND the detected BCP-47 locale (used to pin the answer language).
#[derive(Clone)]
pub struct TranslationService {
    llm: Arc<dyn LlmProvider>,
}

impl TranslationService {
    pub fn new(llm: Arc<dyn LlmProvider>) -> Self {
        Self { llm }
    }

    /// Run the LLM translator/normalizer/locale-detector on `query`.
    ///
    /// Returns the translation result alongside the optional raw
    /// [`LlmUsage`] emitted by the provider. Callers that don't care
    /// about accounting can destructure and drop the second element.
    #[instrument(skip(self))]
    pub async fn translate(&self, query: &str) -> Result<(TranslationResult, Option<LlmUsage>)> {
        self.llm.translate(query).await
    }
}
