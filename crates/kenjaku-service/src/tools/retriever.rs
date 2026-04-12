use std::sync::Arc;

use async_trait::async_trait;
use tracing::instrument;

use kenjaku_core::error::Result;
use kenjaku_core::traits::embedding::EmbeddingProvider;
use kenjaku_core::traits::retriever::{Reranker, Retriever};
use kenjaku_core::types::search::RetrievedChunk;
use kenjaku_infra::qdrant::QdrantClient;

use crate::reranker::RrfReranker;

/// Hybrid retriever combining vector search and full-text search.
pub struct HybridRetriever {
    qdrant: QdrantClient,
    embedding_provider: Arc<dyn EmbeddingProvider>,
    reranker: RrfReranker,
    semantic_weight: f32,
    bm25_weight: f32,
    over_retrieve_factor: usize,
}

impl HybridRetriever {
    pub fn new(
        qdrant: QdrantClient,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        semantic_weight: f32,
        bm25_weight: f32,
        over_retrieve_factor: usize,
    ) -> Self {
        Self {
            qdrant,
            embedding_provider,
            reranker: RrfReranker::new(),
            semantic_weight,
            bm25_weight,
            over_retrieve_factor,
        }
    }
}

#[async_trait]
impl Retriever for HybridRetriever {
    #[instrument(skip(self), fields(collection = %collection, top_k = top_k))]
    async fn retrieve(
        &self,
        query: &str,
        collection: &str,
        top_k: usize,
    ) -> Result<Vec<RetrievedChunk>> {
        let over_retrieve_k = top_k * self.over_retrieve_factor;

        // Embed the query
        let embeddings = self.embedding_provider.embed(&[query.to_string()]).await?;
        let query_embedding = embeddings.into_iter().next().ok_or_else(|| {
            kenjaku_core::error::Error::Embedding("No embedding returned".to_string())
        })?;

        // Run vector and full-text search in parallel
        let (vector_results, fulltext_results) = tokio::try_join!(
            self.qdrant.vector_search(query_embedding, over_retrieve_k),
            self.qdrant.fulltext_search(query, over_retrieve_k),
        )?;

        // Merge with RRF
        let results = self.reranker.rerank(
            &vector_results,
            &fulltext_results,
            top_k,
            self.semantic_weight,
            self.bm25_weight,
        );

        Ok(results)
    }
}
