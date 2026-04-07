use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use futures::Stream;
use tracing::{info, instrument, warn};

use kenjaku_core::error::Result;
use kenjaku_core::traits::intent::IntentClassifier;
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::traits::retriever::Retriever;
use kenjaku_core::types::component::SuggestionSource;
use kenjaku_core::types::conversation::CreateConversation;
use kenjaku_core::types::intent::Intent;
use kenjaku_core::types::search::{
    SearchMetadata, SearchRequest, SearchResponse, StreamChunk,
};

use crate::component::ComponentService;
use crate::conversation::ConversationService;
use crate::translation::TranslationService;
use crate::trending::TrendingService;

/// Orchestrates the full RAG search pipeline.
pub struct SearchService {
    retriever: Arc<dyn Retriever>,
    llm: Arc<dyn LlmProvider>,
    intent_classifier: Arc<dyn IntentClassifier>,
    component_service: ComponentService,
    translation_service: TranslationService,
    trending_service: TrendingService,
    conversation_service: ConversationService,
    collection_name: String,
    suggestion_count: usize,
}

impl SearchService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        retriever: Arc<dyn Retriever>,
        llm: Arc<dyn LlmProvider>,
        intent_classifier: Arc<dyn IntentClassifier>,
        component_service: ComponentService,
        translation_service: TranslationService,
        trending_service: TrendingService,
        conversation_service: ConversationService,
        collection_name: String,
        suggestion_count: usize,
    ) -> Self {
        Self {
            retriever,
            llm,
            intent_classifier,
            component_service,
            translation_service,
            trending_service,
            conversation_service,
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

        // Step 1 + 2: Classify intent AND translate/normalize query in parallel.
        // Both are independent LLM calls, so we save ~1s by issuing them together.
        let (intent_result, translate_result) = tokio::join!(
            self.intent_classifier.classify(&req.query),
            self.translation_service.translate(&req.query),
        );

        let intent = match intent_result {
            Ok(classification) => {
                info!(
                    intent = %classification.intent,
                    confidence = classification.confidence,
                    "Query intent classified"
                );
                classification.intent
            }
            Err(e) => {
                warn!(error = %e, "Intent classification failed, defaulting to Unknown");
                Intent::Unknown
            }
        };

        let search_query = match translate_result {
            Ok(normalized) => normalized,
            Err(e) => {
                warn!(error = %e, "Query normalization failed, falling back to raw query");
                req.query.clone()
            }
        };
        let translated = if search_query != req.query {
            Some(search_query.clone())
        } else {
            None
        };

        // Step 3: Retrieve with hybrid search
        let chunks = self
            .retriever
            .retrieve(&search_query, &self.collection_name, req.top_k)
            .await?;

        // Step 4: Generate LLM response
        let llm_response = self.llm.generate(&search_query, &chunks).await?;

        // Step 5: Get suggestions (LLM first, fallback to Qdrant titles)
        let suggestions = match self
            .llm
            .suggest(&search_query, &llm_response.answer)
            .await
        {
            Ok(s) if s.len() >= self.suggestion_count => s[..self.suggestion_count].to_vec(),
            _ => {
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

        // Step 6: Assemble components
        let components = self.component_service.assemble(
            &llm_response,
            suggestions,
            suggestion_source,
        );

        // Step 7: Record trending (fire-and-forget)
        let _ = self
            .trending_service
            .record_query(req.locale.as_str(), &req.query)
            .await;

        let latency_ms = start.elapsed().as_millis() as u64;

        let response = SearchResponse {
            request_id: req.request_id.clone(),
            session_id: req.session_id.clone(),
            components,
            metadata: SearchMetadata {
                original_query: req.query.clone(),
                translated_query: translated,
                locale: req.locale,
                intent,
                retrieval_count: chunks.len(),
                latency_ms,
            },
        };

        // Step 8: Queue conversation for async persistence (fire-and-forget)
        let meta = match serde_json::to_value(&response) {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "Failed to serialize search response for conversation meta");
                serde_json::json!({ "serialization_error": e.to_string() })
            }
        };
        self.conversation_service
            .record(CreateConversation {
                session_id: req.session_id.clone(),
                request_id: req.request_id.clone(),
                query: req.query.clone(),
                response_text: response
                    .components
                    .iter()
                    .find_map(|c| {
                        if let kenjaku_core::types::component::Component::LlmAnswer(a) = c {
                            Some(a.answer.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default(),
                locale: req.locale,
                intent,
                meta,
            })
            .await;

        info!(
            request_id = %req.request_id,
            latency_ms = latency_ms,
            intent = %intent,
            chunks_retrieved = chunks.len(),
            "Search completed"
        );

        Ok(response)
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
        // Step 1 + 2: Classify intent AND translate/normalize in parallel.
        let (intent_result, translate_result) = tokio::join!(
            self.intent_classifier.classify(&req.query),
            self.translation_service.translate(&req.query),
        );
        let _intent = intent_result.map(|c| c.intent).unwrap_or(Intent::Unknown);
        let search_query = translate_result.unwrap_or_else(|_| req.query.clone());

        // Step 3: Retrieve
        let chunks = self
            .retriever
            .retrieve(&search_query, &self.collection_name, req.top_k)
            .await?;

        // Step 4: Stream LLM response
        let stream = self.llm.generate_stream(&search_query, &chunks).await?;

        // Step 5: Record trending
        let _ = self
            .trending_service
            .record_query(req.locale.as_str(), &req.query)
            .await;

        Ok(stream)
    }
}
