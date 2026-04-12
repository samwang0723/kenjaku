pub mod context;
pub mod fanout;

use std::sync::Arc;
use std::time::Instant;

use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};

use kenjaku_core::config::WebSearchConfig;
use kenjaku_core::error::Result;
use kenjaku_core::traits::intent::IntentClassifier;
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::traits::tool::Tool;
use kenjaku_core::types::component::SuggestionSource;
use kenjaku_core::types::conversation::{ConversationTurn, CreateConversation};
use kenjaku_core::types::intent::Intent;
use kenjaku_core::types::search::{
    LlmSource, SearchMetadata, SearchRequest, SearchResponse, StreamDoneMetadata,
    StreamStartMetadata,
};
use kenjaku_core::types::tool::ToolRequest;

use crate::component::ComponentService;
use crate::conversation::ConversationService;
use crate::history::SessionHistoryStore;
use crate::locale_memory::LocaleMemory;
use crate::quality::prettify_title;
use crate::search::{resolve_translation, SearchStreamOutput, StreamContext};
use crate::translation::TranslationService;
use crate::trending::TrendingService;

/// Internal orchestrator behind `SearchService`. Owns the full RAG pipeline
/// but is not exported to the API crate.
pub(crate) struct SearchOrchestrator {
    llm: Arc<dyn LlmProvider>,
    intent_classifier: Arc<dyn IntentClassifier>,
    component_service: ComponentService,
    translation_service: TranslationService,
    trending_service: TrendingService,
    conversation_service: ConversationService,
    title_resolver: Option<Arc<kenjaku_infra::title_resolver::TitleResolver>>,
    locale_memory: Arc<LocaleMemory>,
    history_store: SessionHistoryStore,
    /// Ordered tool list: [DocRagTool, BraveWebTool, ...].
    /// Tier 1 = index 0; tier 2+ = indices 1..
    tools: Vec<Arc<dyn Tool>>,
    collection_name: String,
    suggestion_count: usize,
    /// Per-tool timeout in milliseconds.
    tool_budget_ms: u64,
}

impl SearchOrchestrator {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        llm: Arc<dyn LlmProvider>,
        intent_classifier: Arc<dyn IntentClassifier>,
        component_service: ComponentService,
        translation_service: TranslationService,
        trending_service: TrendingService,
        conversation_service: ConversationService,
        title_resolver: Option<Arc<kenjaku_infra::title_resolver::TitleResolver>>,
        locale_memory: Arc<LocaleMemory>,
        history_store: SessionHistoryStore,
        tools: Vec<Arc<dyn Tool>>,
        web_search_config: &WebSearchConfig,
        collection_name: String,
        suggestion_count: usize,
    ) -> Self {
        Self {
            llm,
            intent_classifier,
            component_service,
            translation_service,
            trending_service,
            conversation_service,
            title_resolver,
            locale_memory,
            history_store,
            tools,
            collection_name,
            suggestion_count,
            tool_budget_ms: web_search_config.timeout_ms,
        }
    }

    /// Execute a non-streaming search.
    #[instrument(skip(self), fields(
        request_id = %req.request_id,
    ))]
    pub(crate) async fn search(
        &self,
        req: &SearchRequest,
        device_session_id: Option<&str>,
    ) -> Result<SearchResponse> {
        let start = Instant::now();
        let cancel = CancellationToken::new();

        // Step 1 + 2: Classify intent AND translate/normalize+detect-locale
        // in parallel. Both are independent LLM calls, so we save ~1s by
        // issuing them together.
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

        let (search_query, detected_locale, locale_source) =
            resolve_translation(&req.query, translate_result);
        let translated = if search_query != req.query {
            Some(search_query.clone())
        } else {
            None
        };

        info!(
            detected_locale = %detected_locale,
            source = ?locale_source,
            "Resolved answer locale"
        );

        // Fire-and-forget: record the detected locale into LocaleMemory so
        // subsequent same-session reads (autocomplete, top-searches) can
        // honor it without requiring a client hint.
        {
            let lm = self.locale_memory.clone();
            let sid = device_session_id
                .map(str::to_owned)
                .unwrap_or_else(|| req.session_id.clone());
            tokio::spawn(async move {
                lm.record(&sid, detected_locale).await;
            });
        }

        // Step 3 + 3b: Retrieve via tool fan-out (DocRag tier-1, Brave tier-2).
        let tool_req = ToolRequest {
            query_raw: req.query.clone(),
            query_normalized: search_query.clone(),
            locale: detected_locale,
            intent,
            collection_name: self.collection_name.clone(),
            top_k: req.top_k,
            request_id: req.request_id.clone(),
            session_id: req.session_id.clone(),
        };

        let tool_outputs =
            fanout::fanout_tools(&self.tools, &tool_req, &cancel, self.tool_budget_ms).await?;

        let grounding = context::grounding_from_outputs(&tool_outputs);
        let chunks = context::merge_tool_outputs(&tool_outputs);

        // Session-scoped conversation history for follow-up context.
        let history_key = device_session_id.unwrap_or(&req.session_id);
        let history = self.history_store.snapshot_for_llm(history_key);

        // Step 4: Generate LLM response in the detected locale.
        let llm_response = self
            .llm
            .generate(&search_query, &chunks, &history, detected_locale)
            .await?;

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

        // Step 7: Record trending (fire-and-forget) under the DETECTED
        // locale, not a client hint.
        let _ = self
            .trending_service
            .record_query(detected_locale, &req.query, &search_query)
            .await;

        let latency_ms = start.elapsed().as_millis() as u64;

        let response = SearchResponse {
            request_id: req.request_id.clone(),
            session_id: req.session_id.clone(),
            components,
            metadata: SearchMetadata {
                original_query: req.query.clone(),
                translated_query: translated,
                locale: detected_locale,
                detected_locale_source: locale_source,
                intent,
                retrieval_count: chunks.len(),
                latency_ms,
                grounding: grounding.clone(),
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
        let answer_text = response
            .components
            .iter()
            .find_map(|c| {
                if let kenjaku_core::types::component::Component::LlmAnswer(a) = c {
                    Some(a.answer.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        self.conversation_service
            .record(CreateConversation {
                session_id: req.session_id.clone(),
                request_id: req.request_id.clone(),
                query: req.query.clone(),
                response_text: answer_text.clone(),
                locale: detected_locale,
                intent,
                meta,
            })
            .await;

        // Append to in-memory session history so the next turn from the
        // same device can see this exchange.
        if !answer_text.is_empty() {
            self.history_store.append(
                history_key,
                ConversationTurn {
                    user: req.query.clone(),
                    assistant: answer_text,
                },
            );
        }

        info!(
            request_id = %req.request_id,
            latency_ms = latency_ms,
            intent = %intent,
            chunks_retrieved = chunks.len(),
            history_turns = history.len(),
            "Search completed"
        );

        Ok(response)
    }

    /// Execute a streaming search.
    #[instrument(skip(self), fields(
        request_id = %req.request_id,
    ))]
    pub(crate) async fn search_stream(
        &self,
        req: &SearchRequest,
        device_session_id: Option<&str>,
    ) -> Result<SearchStreamOutput> {
        let start = Instant::now();
        let cancel = CancellationToken::new();

        // Step 1 + 2: Classify intent AND translate/normalize+detect-locale
        // in parallel.
        let (intent_result, translate_result) = tokio::join!(
            self.intent_classifier.classify(&req.query),
            self.translation_service.translate(&req.query),
        );
        let intent = intent_result.map(|c| c.intent).unwrap_or(Intent::Unknown);
        let (search_query, detected_locale, locale_source) =
            resolve_translation(&req.query, translate_result);
        let translated_query = if search_query != req.query {
            Some(search_query.clone())
        } else {
            None
        };

        info!(
            detected_locale = %detected_locale,
            source = ?locale_source,
            "Resolved answer locale (streaming)"
        );

        // Fire-and-forget: record detected locale for the session.
        {
            let lm = self.locale_memory.clone();
            let sid = device_session_id
                .map(str::to_owned)
                .unwrap_or_else(|| req.session_id.clone());
            tokio::spawn(async move {
                lm.record(&sid, detected_locale).await;
            });
        }

        // Step 3 + 3b: Retrieve via tool fan-out.
        let tool_req = ToolRequest {
            query_raw: req.query.clone(),
            query_normalized: search_query.clone(),
            locale: detected_locale,
            intent,
            collection_name: self.collection_name.clone(),
            top_k: req.top_k,
            request_id: req.request_id.clone(),
            session_id: req.session_id.clone(),
        };

        let tool_outputs =
            fanout::fanout_tools(&self.tools, &tool_req, &cancel, self.tool_budget_ms).await?;

        let grounding = context::grounding_from_outputs(&tool_outputs);
        let chunks = context::merge_tool_outputs(&tool_outputs);

        // Sources are known at this point. Dedupe by URL and prettify titles.
        let mut seen_urls = std::collections::HashSet::new();
        let sources: Vec<LlmSource> = chunks
            .iter()
            .filter_map(|c| {
                let url = c.source_url.as_ref()?;
                if !seen_urls.insert(url.clone()) {
                    return None;
                }
                Some(LlmSource {
                    title: prettify_title(&c.title),
                    url: url.clone(),
                    snippet: None,
                })
            })
            .collect();

        // Pull session history for follow-up context.
        let history_key = device_session_id.unwrap_or(&req.session_id);
        let history = self.history_store.snapshot_for_llm(history_key);

        // Step 4: Open the LLM stream pinned to the detected locale.
        let stream = self
            .llm
            .generate_stream(&search_query, &chunks, &history, detected_locale)
            .await?;

        // Step 5: Record trending (fire-and-forget).
        let _ = self
            .trending_service
            .record_query(detected_locale, &req.query, &search_query)
            .await;

        let preamble_latency_ms = start.elapsed().as_millis() as u64;

        let start_metadata = StreamStartMetadata {
            request_id: req.request_id.clone(),
            session_id: req.session_id.clone(),
            original_query: req.query.clone(),
            translated_query,
            locale: detected_locale,
            detected_locale_source: locale_source,
            intent,
            retrieval_count: chunks.len(),
            preamble_latency_ms,
            grounding: grounding.clone(),
        };

        Ok(SearchStreamOutput {
            start_metadata,
            stream,
            context: StreamContext {
                sources,
                llm_model: self.llm_model_name(),
                start_instant: start,
                grounding,
                request_id: req.request_id.clone(),
                session_id: req.session_id.clone(),
                history_key: history_key.to_string(),
                query: req.query.clone(),
                locale: detected_locale,
                intent,
            },
        })
    }

    /// Called by the handler after the token stream finishes. Produces the
    /// final `done` metadata and queues the conversation for async
    /// persistence.
    pub(crate) async fn complete_stream(
        &self,
        ctx: StreamContext,
        accumulated_answer: &str,
        grounding_sources: Vec<LlmSource>,
    ) -> StreamDoneMetadata {
        let grounding_sources_was_empty = grounding_sources.is_empty();
        let suggestions = match self.llm.suggest(&ctx.query, accumulated_answer).await {
            Ok(s) if s.len() >= self.suggestion_count => s[..self.suggestion_count].to_vec(),
            Ok(s) => s,
            Err(_) => Vec::new(),
        };

        // Resolve grounding URLs in parallel (cache-backed) into real titles.
        let resolved_grounding = if grounding_sources.is_empty() {
            Vec::new()
        } else if let Some(resolver) = self.title_resolver.as_ref() {
            resolver.resolve_batch(grounding_sources).await
        } else {
            grounding_sources
        };

        // Order: google_search grounding sources first, then internal chunk
        // sources. Deduped by URL.
        let mut merged_sources: Vec<LlmSource> =
            Vec::with_capacity(resolved_grounding.len() + ctx.sources.len());
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for src in resolved_grounding {
            if seen.insert(src.url.clone()) {
                merged_sources.push(src);
            }
        }
        for src in &ctx.sources {
            if seen.insert(src.url.clone()) {
                merged_sources.push(src.clone());
            }
        }

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
                    "sources": merged_sources,
                    "suggestions": suggestions,
                    "streaming": true,
                }),
            })
            .await;

        // Append to in-memory session history (streaming path).
        if !accumulated_answer.is_empty() {
            self.history_store.append(
                &ctx.history_key,
                ConversationTurn {
                    user: ctx.query.clone(),
                    assistant: accumulated_answer.to_string(),
                },
            );
        }

        let mut grounding = ctx.grounding;
        if !grounding_sources_was_empty {
            grounding.gemini_grounding_used = true;
        }

        StreamDoneMetadata {
            latency_ms,
            sources: merged_sources,
            suggestions,
            llm_model: ctx.llm_model,
            grounding,
        }
    }

    fn llm_model_name(&self) -> String {
        "gemini".to_string()
    }
}
