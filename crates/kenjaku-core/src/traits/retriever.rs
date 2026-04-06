use async_trait::async_trait;

use crate::error::Result;
use crate::types::search::RetrievedChunk;

/// Trait for document retrieval from a vector/search store.
#[async_trait]
pub trait Retriever: Send + Sync {
    /// Retrieve the top-k most relevant chunks for a query.
    async fn retrieve(
        &self,
        query: &str,
        collection: &str,
        top_k: usize,
    ) -> Result<Vec<RetrievedChunk>>;
}

/// Trait for reranking retrieved chunks.
#[async_trait]
pub trait Reranker: Send + Sync {
    /// Rerank chunks using Reciprocal Rank Fusion or another strategy.
    /// Takes vector results and full-text results, returns merged + reranked results.
    fn rerank(
        &self,
        vector_results: &[RetrievedChunk],
        fulltext_results: &[RetrievedChunk],
        top_k: usize,
        semantic_weight: f32,
        bm25_weight: f32,
    ) -> Vec<RetrievedChunk>;
}
