use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use futures::Stream;
use tracing::{info, instrument};

use kenjaku_core::error::Result;
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::traits::retriever::Retriever;
use kenjaku_core::types::component::SuggestionSource;
use kenjaku_core::types::search::{
    SearchMetadata, SearchRequest, SearchResponse, StreamChunk,
};

use crate::component::ComponentService;
use crate::translation::TranslationService;
use crate::trending::TrendingService;

/// Orchestrates the full RAG search pipeline.
pub struct SearchService {
    retriever: Arc<dyn Retriever>,
    llm: Arc<dyn LlmProvider>,
    component_service: ComponentService,
    translation_service: TranslationService,
    trending_service: TrendingService,
    collection_name: String,
    suggestion_count: usize,
}

impl SearchService {
    pub fn new(
        retriever: Arc<dyn Retriever>,
        llm: Arc<dyn LlmProvider>,
        component_service: ComponentService,
        translation_service: TranslationService,
        trending_service: TrendingService,
        collection_name: String,
        suggestion_count: usize,
    ) -> Self {
        Self {
            retriever,
            llm,
            component_service,
            translation_service,
            trending_service,
            collection_name,
            suggestion_count,
        }
    }

    /// Execute a non-streaming search.
    #[instrument(skip(self), fields(
        request_id = %req.request_id,
        locale = %req.locale,
    ))]
    pub async fn search(&self, req: &SearchRequest) -> Result<SearchResponse> {
        let start = Instant::now();

        // Step 1: Translate if needed
        let (search_query, translated) = if req.locale != "en" {
            let translated = self
                .translation_service
                .translate(&req.query, &req.locale)
                .await?;
            (translated.clone(), Some(translated))
        } else {
            (req.query.clone(), None)
        };

        // Step 2: Retrieve with hybrid search
        let chunks = self
            .retriever
            .retrieve(&search_query, &self.collection_name, req.top_k)
            .await?;

        // Step 3: Generate LLM response
        let llm_response = self.llm.generate(&search_query, &chunks).await?;

        // Step 4: Get suggestions (LLM first, fallback to Qdrant titles)
        let suggestions = match self
            .llm
            .suggest(&search_query, &llm_response.answer)
            .await
        {
            Ok(s) if s.len() >= self.suggestion_count => s[..self.suggestion_count].to_vec(),
            _ => {
                // Fallback: use Qdrant document titles
                chunks
                    .iter()
                    .map(|c| c.title.clone())
                    .take(self.suggestion_count)
                    .collect()
            }
        };

        let suggestion_source = if suggestions.len() == self.suggestion_count {
            SuggestionSource::Llm
        } else {
            SuggestionSource::VectorStore
        };

        // Step 5: Assemble components
        let components = self.component_service.assemble(
            &llm_response,
            suggestions,
            suggestion_source,
        );

        // Step 6: Record trending
        let _ = self
            .trending_service
            .record_query(&req.locale, &req.query)
            .await;

        let latency_ms = start.elapsed().as_millis() as u64;

        info!(
            request_id = %req.request_id,
            latency_ms = latency_ms,
            chunks_retrieved = chunks.len(),
            "Search completed"
        );

        Ok(SearchResponse {
            request_id: req.request_id.clone(),
            session_id: req.session_id.clone(),
            components,
            metadata: SearchMetadata {
                original_query: req.query.clone(),
                translated_query: translated,
                locale: req.locale.clone(),
                retrieval_count: chunks.len(),
                latency_ms,
            },
        })
    }

    /// Execute a streaming search (returns SSE stream).
    #[instrument(skip(self), fields(
        request_id = %req.request_id,
        locale = %req.locale,
    ))]
    pub async fn search_stream(
        &self,
        req: &SearchRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        // Step 1: Translate if needed
        let search_query = if req.locale != "en" {
            self.translation_service
                .translate(&req.query, &req.locale)
                .await?
        } else {
            req.query.clone()
        };

        // Step 2: Retrieve
        let chunks = self
            .retriever
            .retrieve(&search_query, &self.collection_name, req.top_k)
            .await?;

        // Step 3: Stream LLM response
        let stream = self.llm.generate_stream(&search_query, &chunks).await?;

        // Step 4: Record trending
        let _ = self
            .trending_service
            .record_query(&req.locale, &req.query)
            .await;

        Ok(stream)
    }
}
