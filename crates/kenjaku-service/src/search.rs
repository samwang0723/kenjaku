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
    LlmSource, SearchMetadata, SearchRequest, SearchResponse, StreamChunk, StreamDoneMetadata,
    StreamStartMetadata,
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
        let suggestions = match self.llm.suggest(&search_query, &llm_response.answer).await {
            Ok(s) if s.len() >= self.suggestion_count => s[..self.suggestion_count].to_vec(),
            _ => chunks
                .iter()
                .map(|c| c.title.clone())
                .take(self.suggestion_count)
                .collect(),
        };

        let suggestion_source = if suggestions.len() == self.suggestion_count {
            SuggestionSource::Llm
        } else {
            SuggestionSource::VectorStore
        };

        // Step 6: Assemble components
        let components =
            self.component_service
                .assemble(&llm_response, suggestions, suggestion_source);

        // Step 7: Record trending (fire-and-forget). The trending service
        // applies a gibberish guard and normalizes the stored form.
        let _ = self
            .trending_service
            .record_query(req.locale, &req.query, &search_query)
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

    /// Execute a streaming search.
    ///
    /// Returns a `SearchStreamOutput` containing:
    /// - `start_metadata` — everything we know BEFORE the LLM stream begins
    ///   (intent, translated_query, retrieval_count, preamble_latency_ms, sources).
    ///   Emit this as an SSE `start` event for instant debug panel population.
    /// - `stream` — the token delta stream (raw `StreamChunk`s from the LLM).
    /// - `llm_model` — for final `done` metadata.
    ///
    /// Suggestions are NOT computed here (they would require waiting for the full
    /// answer, defeating streaming). The handler can call `complete_stream` after
    /// the token stream finishes, passing in the accumulated answer, to get final
    /// `done` metadata and queue the conversation record.
    #[instrument(skip(self), fields(
        request_id = %req.request_id,
        locale = %req.locale,
    ))]
    pub async fn search_stream(&self, req: &SearchRequest) -> Result<SearchStreamOutput> {
        let start = Instant::now();

        // Step 1 + 2: Classify intent AND translate/normalize in parallel.
        let (intent_result, translate_result) = tokio::join!(
            self.intent_classifier.classify(&req.query),
            self.translation_service.translate(&req.query),
        );
        let intent = intent_result.map(|c| c.intent).unwrap_or(Intent::Unknown);
        let search_query = translate_result.unwrap_or_else(|_| req.query.clone());
        let translated_query = if search_query != req.query {
            Some(search_query.clone())
        } else {
            None
        };

        // Step 3: Retrieve
        let chunks = self
            .retriever
            .retrieve(&search_query, &self.collection_name, req.top_k)
            .await?;

        // Sources are known at this point (from retrieved chunks).
        let sources: Vec<LlmSource> = chunks
            .iter()
            .filter_map(|c| {
                c.source_url.as_ref().map(|url| LlmSource {
                    title: c.title.clone(),
                    url: url.clone(),
                    snippet: None,
                })
            })
            .collect();

        // Step 4: Open the LLM stream
        let stream = self.llm.generate_stream(&search_query, &chunks).await?;

        // Step 5: Record trending (fire-and-forget). The trending service
        // applies a gibberish guard and normalizes the stored form.
        let _ = self
            .trending_service
            .record_query(req.locale, &req.query, &search_query)
            .await;

        let preamble_latency_ms = start.elapsed().as_millis() as u64;

        let start_metadata = StreamStartMetadata {
            request_id: req.request_id.clone(),
            session_id: req.session_id.clone(),
            original_query: req.query.clone(),
            translated_query,
            locale: req.locale,
            intent,
            retrieval_count: chunks.len(),
            preamble_latency_ms,
        };

        Ok(SearchStreamOutput {
            start_metadata,
            stream,
            context: StreamContext {
                sources,
                llm_model: self.llm_model_name(),
                start_instant: start,
                request_id: req.request_id.clone(),
                session_id: req.session_id.clone(),
                query: req.query.clone(),
                locale: req.locale,
                intent,
            },
        })
    }

    /// Called by the handler after the token stream finishes. Produces the final
    /// `done` metadata and queues the conversation for async persistence.
    ///
    /// Takes ownership of the `ctx` because the stream field isn't needed after
    /// this point, and the caller has already drained it.
    pub async fn complete_stream(
        &self,
        ctx: StreamContext,
        accumulated_answer: &str,
    ) -> StreamDoneMetadata {
        let suggestions = match self.llm.suggest(&ctx.query, accumulated_answer).await {
            Ok(s) if s.len() >= self.suggestion_count => s[..self.suggestion_count].to_vec(),
            Ok(s) => s,
            Err(_) => Vec::new(),
        };

        let latency_ms = ctx.start_instant.elapsed().as_millis() as u64;

        self.conversation_service
            .record(CreateConversation {
                session_id: ctx.session_id.clone(),
                request_id: ctx.request_id.clone(),
                query: ctx.query.clone(),
                response_text: accumulated_answer.to_string(),
                locale: ctx.locale,
                intent: ctx.intent,
                meta: serde_json::json!({
                    "latency_ms": latency_ms,
                    "sources": ctx.sources,
                    "suggestions": suggestions,
                    "streaming": true,
                }),
            })
            .await;

        StreamDoneMetadata {
            latency_ms,
            sources: ctx.sources,
            suggestions,
            llm_model: ctx.llm_model,
        }
    }

    fn llm_model_name(&self) -> String {
        // Trait doesn't expose model name directly; use a placeholder.
        // Providers encode the actual model into LlmResponse.model on non-stream.
        "gemini".to_string()
    }
}

/// Output of `SearchService::search_stream`. The handler drains `stream`,
/// then calls `complete_stream(context, accumulated_answer)` to get the final
/// `done` metadata.
pub struct SearchStreamOutput {
    pub start_metadata: StreamStartMetadata,
    pub stream: Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>,
    pub context: StreamContext,
}

/// Bookkeeping passed from `search_stream` to `complete_stream` — kept
/// separate from the Stream so the handler can move it into the completion
/// call after dropping the stream.
pub struct StreamContext {
    pub sources: Vec<LlmSource>,
    pub llm_model: String,
    pub start_instant: Instant,
    pub request_id: String,
    pub session_id: String,
    pub query: String,
    pub locale: kenjaku_core::types::locale::Locale,
    pub intent: Intent,
}
