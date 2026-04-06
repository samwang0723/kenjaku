use async_trait::async_trait;

use crate::error::Result;

/// Trait for embedding providers. Implementations can use OpenAI, Cohere, local models, etc.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate embeddings for a batch of texts.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// The name of the embedding model.
    fn model_name(&self) -> &str;

    /// The dimensionality of the embedding vectors.
    fn dimensions(&self) -> usize;
}
