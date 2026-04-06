use kenjaku_core::traits::retriever::Reranker;
use kenjaku_core::types::search::{RetrievalMethod, RetrievedChunk};

/// Reciprocal Rank Fusion reranker.
/// Merges vector search and full-text search results using weighted RRF.
pub struct RrfReranker;

impl RrfReranker {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RrfReranker {
    fn default() -> Self {
        Self::new()
    }
}

impl Reranker for RrfReranker {
    fn rerank(
        &self,
        vector_results: &[RetrievedChunk],
        fulltext_results: &[RetrievedChunk],
        top_k: usize,
        semantic_weight: f32,
        bm25_weight: f32,
    ) -> Vec<RetrievedChunk> {
        use std::collections::HashMap;

        // Build a map of chunk_id -> (best chunk data, rrf score)
        let mut scores: HashMap<String, (RetrievedChunk, f32)> = HashMap::new();

        // Score vector results
        for (rank, chunk) in vector_results.iter().enumerate() {
            let rrf_score = semantic_weight / (rank as f32 + 1.0);
            scores
                .entry(chunk.chunk_id.clone())
                .and_modify(|(_, s)| *s += rrf_score)
                .or_insert_with(|| (chunk.clone(), rrf_score));
        }

        // Score full-text results
        for (rank, chunk) in fulltext_results.iter().enumerate() {
            let rrf_score = bm25_weight / (rank as f32 + 1.0);
            scores
                .entry(chunk.chunk_id.clone())
                .and_modify(|(existing, s)| {
                    *s += rrf_score;
                    existing.retrieval_method = RetrievalMethod::Hybrid;
                })
                .or_insert_with(|| (chunk.clone(), rrf_score));
        }

        // Sort by score descending
        let mut results: Vec<(RetrievedChunk, f32)> = scores.into_values().collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Take top_k and update scores
        results
            .into_iter()
            .take(top_k)
            .map(|(mut chunk, score)| {
                chunk.score = score;
                chunk
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_chunk(id: &str, score: f32, method: RetrievalMethod) -> RetrievedChunk {
        RetrievedChunk {
            doc_id: "doc1".to_string(),
            chunk_id: id.to_string(),
            title: format!("Chunk {id}"),
            original_content: format!("Content of {id}"),
            contextualized_content: format!("Context of {id}"),
            source_url: None,
            score,
            retrieval_method: method,
        }
    }

    #[test]
    fn test_rrf_reranker_merges_results() {
        let reranker = RrfReranker::new();

        let vector_results = vec![
            make_chunk("a", 0.9, RetrievalMethod::Vector),
            make_chunk("b", 0.8, RetrievalMethod::Vector),
            make_chunk("c", 0.7, RetrievalMethod::Vector),
        ];

        let fulltext_results = vec![
            make_chunk("b", 5.0, RetrievalMethod::FullText),
            make_chunk("d", 4.0, RetrievalMethod::FullText),
            make_chunk("a", 3.0, RetrievalMethod::FullText),
        ];

        let results = reranker.rerank(&vector_results, &fulltext_results, 3, 0.8, 0.2);

        assert_eq!(results.len(), 3);
        // "a" should rank high: 0.8/1 (vector rank 0) + 0.2/3 (fulltext rank 2) = 0.867
        // "b" should rank high: 0.8/2 (vector rank 1) + 0.2/1 (fulltext rank 0) = 0.6
        assert_eq!(results[0].chunk_id, "a");
        assert_eq!(results[1].chunk_id, "b");
        // "b" should be marked as Hybrid since it appeared in both
        assert_eq!(results[1].retrieval_method, RetrievalMethod::Hybrid);
    }

    #[test]
    fn test_rrf_reranker_empty_inputs() {
        let reranker = RrfReranker::new();
        let results = reranker.rerank(&[], &[], 5, 0.8, 0.2);
        assert!(results.is_empty());
    }

    #[test]
    fn test_rrf_reranker_vector_only() {
        let reranker = RrfReranker::new();
        let vector_results = vec![
            make_chunk("a", 0.9, RetrievalMethod::Vector),
            make_chunk("b", 0.8, RetrievalMethod::Vector),
        ];

        let results = reranker.rerank(&vector_results, &[], 2, 0.8, 0.2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].chunk_id, "a");
    }

    #[test]
    fn test_rrf_reranker_respects_top_k() {
        let reranker = RrfReranker::new();
        let vector_results = vec![
            make_chunk("a", 0.9, RetrievalMethod::Vector),
            make_chunk("b", 0.8, RetrievalMethod::Vector),
            make_chunk("c", 0.7, RetrievalMethod::Vector),
        ];

        let results = reranker.rerank(&vector_results, &[], 1, 0.8, 0.2);
        assert_eq!(results.len(), 1);
    }
}
