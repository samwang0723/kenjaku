use async_trait::async_trait;

use crate::error::Result;
use crate::types::intent::IntentClassification;

/// Trait for intent classification providers.
#[async_trait]
pub trait IntentClassifier: Send + Sync {
    /// Classify the intent of a user query.
    async fn classify(&self, query: &str) -> Result<IntentClassification>;
}
