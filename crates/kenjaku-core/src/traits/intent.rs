use async_trait::async_trait;

use crate::error::Result;
use crate::types::intent::IntentClassification;
use crate::types::usage::LlmCall;

/// Trait for intent classification providers.
#[async_trait]
pub trait IntentClassifier: Send + Sync {
    /// Classify the intent of a user query.
    ///
    /// Returns the classification paired with an optional [`LlmCall`]
    /// accounting entry so the pipeline can roll up token usage +
    /// cost across all LLM calls in the request.
    async fn classify(&self, query: &str) -> Result<(IntentClassification, Option<LlmCall>)>;
}
