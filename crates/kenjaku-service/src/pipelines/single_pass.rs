//! Single-pass RAG search pipeline.
//!
//! Extracted from the historical `SearchOrchestrator` god-object. This
//! implementation owns the full classify -> translate -> fan-out ->
//! generate -> assemble flow and is the default pipeline registered in
//! `kenjaku-server/src/main.rs`.
//!
//! Forward-compat notes (tracked in
//! `docs/architecture/flexibility-refactor-tech-spec.md`):
//! - Phase 2 (LANDED): the `Arc<dyn Brain>` is now a `CompositeBrain`
//!   composing `Classifier + Translator + Generator`. The pipeline
//!   signature is unchanged; the `has_web_grounding: bool` and
//!   hardcoded `"gemini"` model-name leaks are gone — both are now
//!   read via `Brain::has_web_grounding()` / `Brain::model_name()`.
//! - Phase 3b (LANDED): `SearchPipeline::search`/`search_stream` now
//!   accept `&TenantContext` and the pipeline forwards it into every
//!   downstream call (tools, trending, conversations, locale memory).
//!   `StreamContext.tenant` persists the context for `complete_stream`.
//!   In 3b every call site resolves to `TenantContext::public()`; slice
//!   3c wires the JWT extractor at the handler boundary.
//! - Phase 3c/3d will decide whether `complete_stream` is promoted onto
//!   the `SearchPipeline` trait. Until then it remains an inherent method
//!   on this concrete type.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};

use kenjaku_core::config::WebSearchConfig;
use kenjaku_core::error::Result;
use kenjaku_core::traits::brain::Brain;
use kenjaku_core::traits::pipeline::SearchPipeline;
use kenjaku_core::traits::tool::Tool;
use kenjaku_core::types::component::SuggestionSource;
use kenjaku_core::types::conversation::{ConversationTurn, CreateConversation};
use kenjaku_core::types::intent::Intent;
use kenjaku_core::types::search::{
    CancelGuard, LlmSource, SearchMetadata, SearchRequest, SearchResponse, SearchStreamOutput,
    StreamContext, StreamDoneMetadata, StreamStartMetadata,
};
use kenjaku_core::types::tenant::TenantContext;
use kenjaku_core::types::tool::ToolRequest;

use crate::brain::ConversationAssembler;
use crate::component::ComponentService;
use crate::conversation::ConversationService;
use crate::harness::{context, fanout::ToolTunnel};
use crate::history::SessionHistoryStore;
use crate::locale_memory::LocaleMemory;
use crate::quality::prettify_title;
use crate::search::resolve_translation;
use crate::trending::TrendingService;

/// Default single-pass RAG pipeline.
///
/// Owns every collaborator needed for a search request. Instances are
/// cheap to clone as `Arc<dyn SearchPipeline>` and are expected to live
/// for the lifetime of the server.
pub struct SinglePassPipeline {
    brain: Arc<dyn Brain>,
    component_service: ComponentService,
    /// `None` only in unit tests where Redis is unavailable.
    trending_service: Option<TrendingService>,
    conversation_service: ConversationService,
    title_resolver: Option<Arc<kenjaku_infra::title_resolver::TitleResolver>>,
    /// `None` only in unit tests where Redis is unavailable.
    locale_memory: Option<Arc<LocaleMemory>>,
    history_store: SessionHistoryStore,
    /// DAG-based tool executor. Tools declare dependencies; the tunnel
    /// resolves execution tiers at construction time via topological sort.
    tunnel: ToolTunnel,
    collection_name: String,
    suggestion_count: usize,
    /// Per-tool timeout in milliseconds.
    tool_budget_ms: u64,
}

impl SinglePassPipeline {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        brain: Arc<dyn Brain>,
        component_service: ComponentService,
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
        let tunnel = ToolTunnel::new(tools);
        Self {
            brain,
            component_service,
            trending_service: Some(trending_service),
            conversation_service,
            title_resolver,
            locale_memory: Some(locale_memory),
            history_store,
            tunnel,
            collection_name,
            suggestion_count,
            tool_budget_ms: web_search_config.timeout_ms,
        }
    }

    /// Called by the handler after the token stream finishes. Produces the
    /// final `done` metadata and queues the conversation for async
    /// persistence.
    ///
    /// Not part of the [`SearchPipeline`] trait in Phase 3b — see trait
    /// docs. The tenant identity is read from `ctx.tenant` so this method
    /// keeps its existing handler-facing signature while still routing
    /// per-tenant.
    #[instrument(skip(self, ctx, accumulated_answer, grounding_sources), fields(
        request_id = %ctx.request_id,
        tenant_id = %ctx.tenant.tenant_id.as_str(),
        plan_tier = ?ctx.tenant.plan_tier,
    ))]
    pub async fn complete_stream(
        &self,
        ctx: StreamContext,
        accumulated_answer: &str,
        grounding_sources: Vec<LlmSource>,
    ) -> StreamDoneMetadata {
        let grounding_sources_was_empty = grounding_sources.is_empty();
        let cancel = CancellationToken::new();
        let suggestions = match self
            .brain
            .suggest(&ctx.query, accumulated_answer, &cancel)
            .await
        {
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
            .record(
                &ctx.tenant,
                CreateConversation {
                    tenant_id: ctx.tenant.tenant_id.as_str().to_string(),
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
                },
            )
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
        self.brain.model_name().to_string()
    }
}

#[async_trait]
impl SearchPipeline for SinglePassPipeline {
    #[instrument(skip(self, tctx), fields(
        request_id = %req.request_id,
        tenant_id = %tctx.tenant_id.as_str(),
        plan_tier = ?tctx.plan_tier,
    ))]
    async fn search(
        &self,
        req: &SearchRequest,
        tctx: &TenantContext,
        device_session_id: Option<&str>,
    ) -> Result<SearchResponse> {
        let start = Instant::now();
        let cancel = CancellationToken::new();

        // Step 1 + 2: Classify intent AND translate/normalize+detect-locale
        // in parallel. Both are independent LLM calls, so we save ~1s by
        // issuing them together.
        let (intent_result, translate_result) = tokio::join!(
            self.brain.classify_intent(&req.query, &cancel),
            self.brain.translate(&req.query, &cancel),
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
        if let Some(lm) = self.locale_memory.clone() {
            let sid = device_session_id
                .map(str::to_owned)
                .unwrap_or_else(|| req.session_id.clone());
            let tctx_owned = tctx.clone();
            tokio::spawn(async move {
                lm.record(&tctx_owned, &sid, detected_locale).await;
            });
        }

        // Step 3 + 3b: Retrieve via tool fan-out (DocRag tier-1, Brave tier-2).
        let tool_req = ToolRequest::new(
            req.query.clone(),
            search_query.clone(),
            detected_locale,
            intent,
            self.collection_name.clone(),
            req.top_k,
            req.request_id.clone(),
            req.session_id.clone(),
            tctx,
        );

        let tool_outputs = self
            .tunnel
            .execute(&tool_req, &cancel, self.tool_budget_ms)
            .await?;

        let grounding = context::grounding_from_outputs(&tool_outputs);
        let chunks = context::merge_tool_outputs(&tool_outputs);

        // Session-scoped conversation history for follow-up context.
        let history_key = device_session_id.unwrap_or(&req.session_id);
        let history = self.history_store.snapshot_for_llm(history_key);

        // Build the message sequence via ConversationAssembler.
        let messages = ConversationAssembler::build(
            &history,
            &search_query,
            detected_locale,
            self.brain.has_web_grounding(),
            &chunks,
        );

        // Step 4: Generate LLM response in the detected locale via Brain.
        let llm_response = self
            .brain
            .generate(&messages, &chunks, detected_locale, &cancel)
            .await?;

        // Step 5: Get suggestions (LLM first, fallback to Qdrant titles)
        let suggestions = match self
            .brain
            .suggest(&search_query, &llm_response.answer, &cancel)
            .await
        {
            Ok(s) if s.len() >= self.suggestion_count => s[..self.suggestion_count].to_vec(),
            Ok(s) => {
                warn!(
                    count = s.len(),
                    needed = self.suggestion_count,
                    "LLM returned fewer suggestions than needed, falling back to chunk titles"
                );
                chunks
                    .iter()
                    .map(|c| c.title.clone())
                    .take(self.suggestion_count)
                    .collect()
            }
            Err(e) => {
                warn!(error = %e, "Suggestion generation failed, falling back to chunk titles");
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
        let components =
            self.component_service
                .assemble(&llm_response, suggestions, suggestion_source);

        // Step 7: Record trending (fire-and-forget) under the DETECTED
        // locale, not a client hint.
        if let Some(ref ts) = self.trending_service {
            let _ = ts
                .record_query(tctx, detected_locale, &req.query, &search_query)
                .await;
        }

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
            .record(
                tctx,
                CreateConversation {
                    tenant_id: tctx.tenant_id.as_str().to_string(),
                    session_id: req.session_id.clone(),
                    request_id: req.request_id.clone(),
                    query: req.query.clone(),
                    response_text: answer_text.clone(),
                    locale: detected_locale,
                    intent,
                    meta,
                },
            )
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

    #[instrument(skip(self, tctx), fields(
        request_id = %req.request_id,
        tenant_id = %tctx.tenant_id.as_str(),
        plan_tier = ?tctx.plan_tier,
    ))]
    async fn search_stream(
        &self,
        req: &SearchRequest,
        tctx: &TenantContext,
        device_session_id: Option<&str>,
    ) -> Result<SearchStreamOutput> {
        let start = Instant::now();
        let cancel = CancellationToken::new();

        // Step 1 + 2: Classify intent AND translate/normalize+detect-locale
        // in parallel.
        let (intent_result, translate_result) = tokio::join!(
            self.brain.classify_intent(&req.query, &cancel),
            self.brain.translate(&req.query, &cancel),
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
        if let Some(lm) = self.locale_memory.clone() {
            let sid = device_session_id
                .map(str::to_owned)
                .unwrap_or_else(|| req.session_id.clone());
            let tctx_owned = tctx.clone();
            tokio::spawn(async move {
                lm.record(&tctx_owned, &sid, detected_locale).await;
            });
        }

        // Step 3 + 3b: Retrieve via tool fan-out.
        let tool_req = ToolRequest::new(
            req.query.clone(),
            search_query.clone(),
            detected_locale,
            intent,
            self.collection_name.clone(),
            req.top_k,
            req.request_id.clone(),
            req.session_id.clone(),
            tctx,
        );

        let tool_outputs = self
            .tunnel
            .execute(&tool_req, &cancel, self.tool_budget_ms)
            .await?;

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

        // Build the message sequence via ConversationAssembler.
        let messages = ConversationAssembler::build(
            &history,
            &search_query,
            detected_locale,
            self.brain.has_web_grounding(),
            &chunks,
        );

        // Step 4: Open the LLM stream pinned to the detected locale via Brain.
        let stream = self
            .brain
            .generate_stream(&messages, &chunks, detected_locale, &cancel)
            .await?;

        // Step 5: Record trending (fire-and-forget).
        if let Some(ref ts) = self.trending_service {
            let _ = ts
                .record_query(tctx, detected_locale, &req.query, &search_query)
                .await;
        }

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
                tenant: tctx.clone(),
                _cancel_guard: CancelGuard::new(cancel),
            },
        })
    }

    fn name(&self) -> &'static str {
        "single_pass"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;

    use async_trait::async_trait;
    use futures::Stream;
    use futures::stream;

    use kenjaku_core::config::HistoryConfig;
    use kenjaku_core::error::Error;
    use kenjaku_core::types::component::{Component, ComponentLayout};
    use kenjaku_core::types::intent::IntentClassification;
    use kenjaku_core::types::locale::{DetectedLocale, Locale};
    use kenjaku_core::types::search::{
        DetectedLocaleSource, GroundingInfo, LlmResponse, RetrievalMethod, RetrievedChunk,
        StreamChunk, StreamChunkType, TranslationResult,
    };
    use kenjaku_core::types::tenant::TenantContext;
    use kenjaku_core::types::tool::{ToolConfig, ToolError, ToolId, ToolOutput, ToolOutputMap};

    // ---- MockBrain -----------------------------------------------------------

    /// Configurable mock Brain for pipeline tests.
    struct MockBrain {
        intent: Intent,
        intent_fail: bool,
        translate_fail: bool,
        translate_locale: Locale,
        answer: String,
        suggestions: Vec<String>,
    }

    impl MockBrain {
        fn new() -> Self {
            Self {
                intent: Intent::Factual,
                intent_fail: false,
                translate_fail: false,
                translate_locale: Locale::En,
                answer: "Mock answer".to_string(),
                suggestions: vec![
                    "Suggestion 1".into(),
                    "Suggestion 2".into(),
                    "Suggestion 3".into(),
                ],
            }
        }

        fn with_intent_fail(mut self) -> Self {
            self.intent_fail = true;
            self
        }

        fn with_translate_fail(mut self) -> Self {
            self.translate_fail = true;
            self
        }

        fn with_translate_locale(mut self, locale: Locale) -> Self {
            self.translate_locale = locale;
            self
        }

        fn with_suggestions(mut self, suggestions: Vec<String>) -> Self {
            self.suggestions = suggestions;
            self
        }
    }

    #[async_trait]
    impl Brain for MockBrain {
        async fn classify_intent(
            &self,
            _query: &str,
            _cancel: &CancellationToken,
        ) -> Result<IntentClassification> {
            if self.intent_fail {
                return Err(Error::Llm("intent classification failed".into()));
            }
            Ok(IntentClassification {
                intent: self.intent,
                confidence: 0.95,
            })
        }

        async fn translate(
            &self,
            query: &str,
            _cancel: &CancellationToken,
        ) -> Result<TranslationResult> {
            if self.translate_fail {
                return Err(Error::Llm("translation failed".into()));
            }
            Ok(TranslationResult {
                normalized: query.to_string(),
                detected_locale: DetectedLocale::Supported(self.translate_locale),
            })
        }

        async fn generate(
            &self,
            _messages: &[kenjaku_core::types::message::Message],
            _chunks: &[RetrievedChunk],
            _locale: Locale,
            _cancel: &CancellationToken,
        ) -> Result<LlmResponse> {
            Ok(LlmResponse {
                answer: self.answer.clone(),
                sources: vec![],
                model: "mock-model".to_string(),
                usage: None,
            })
        }

        async fn generate_stream(
            &self,
            _messages: &[kenjaku_core::types::message::Message],
            _chunks: &[RetrievedChunk],
            _locale: Locale,
            _cancel: &CancellationToken,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
            let chunk = StreamChunk {
                delta: self.answer.clone(),
                chunk_type: StreamChunkType::Answer,
                finished: true,
                grounding: None,
            };
            Ok(Box::pin(stream::iter(vec![Ok(chunk)])))
        }

        async fn suggest(
            &self,
            _query: &str,
            _answer: &str,
            _cancel: &CancellationToken,
        ) -> Result<Vec<String>> {
            Ok(self.suggestions.clone())
        }
    }

    // ---- MockTool (adapted from fanout tests) --------------------------------

    static TOOL_CONFIG: ToolConfig = ToolConfig {
        enabled: true,
        rollout_pct: None,
    };

    struct MockTool {
        tool_id: String,
        output: ToolOutput,
        fire: bool,
    }

    impl MockTool {
        fn new(id: &str) -> Self {
            Self {
                tool_id: id.into(),
                output: ToolOutput::Empty,
                fire: true,
            }
        }

        fn with_output(mut self, output: ToolOutput) -> Self {
            self.output = output;
            self
        }

        fn with_fire(mut self, fire: bool) -> Self {
            self.fire = fire;
            self
        }
    }

    #[async_trait]
    impl kenjaku_core::traits::tool::Tool for MockTool {
        fn id(&self) -> ToolId {
            ToolId(self.tool_id.clone())
        }
        fn config(&self) -> &ToolConfig {
            &TOOL_CONFIG
        }
        fn should_fire(&self, _req: &ToolRequest, _prior: &ToolOutputMap) -> bool {
            self.fire
        }
        async fn invoke(
            &self,
            _req: &ToolRequest,
            _prior: &ToolOutputMap,
            _cancel: &CancellationToken,
        ) -> std::result::Result<ToolOutput, ToolError> {
            Ok(self.output.clone())
        }
    }

    // ---- Helpers -------------------------------------------------------------

    fn make_request() -> SearchRequest {
        SearchRequest {
            query: "test query".into(),
            session_id: "sess-1".into(),
            request_id: "req-1".into(),
            streaming: false,
            top_k: 10,
        }
    }

    fn make_chunk(id: &str, title: &str, url: Option<&str>) -> RetrievedChunk {
        RetrievedChunk {
            doc_id: format!("doc-{id}"),
            chunk_id: format!("chunk-{id}"),
            title: title.to_string(),
            original_content: format!("Content of {id}"),
            contextualized_content: format!("Context of {id}"),
            source_url: url.map(String::from),
            score: 0.9,
            retrieval_method: RetrievalMethod::Vector,
        }
    }

    fn history_config() -> HistoryConfig {
        HistoryConfig {
            enabled: true,
            max_turns_per_session: 10,
            inject_max_turns: 3,
            session_idle_ttl_seconds: 3600,
        }
    }

    /// Build a test pipeline with the given Brain and tools, no infra deps.
    fn make_pipeline(
        brain: impl Brain + 'static,
        tools: Vec<Arc<dyn Tool>>,
    ) -> (
        SinglePassPipeline,
        tokio::sync::mpsc::Receiver<CreateConversation>,
    ) {
        let (conversation_service, conv_rx) = ConversationService::test_channel();
        let component_service = ComponentService::new(ComponentLayout::default());
        let history_store = SessionHistoryStore::new(history_config());
        let tunnel = ToolTunnel::new(tools);

        let pipeline = SinglePassPipeline {
            brain: Arc::new(brain),
            component_service,
            trending_service: None,
            conversation_service,
            title_resolver: None,
            locale_memory: None,
            history_store,
            tunnel,
            collection_name: "test-collection".into(),
            suggestion_count: 3,
            tool_budget_ms: 5000,
        };
        (pipeline, conv_rx)
    }

    // ---- Tests ---------------------------------------------------------------

    #[test]
    fn name_returns_single_pass() {
        let brain = MockBrain::new();
        let (pipeline, _rx) = make_pipeline(brain, vec![]);
        assert_eq!(pipeline.name(), "single_pass");
    }

    #[tokio::test]
    async fn pipeline_search_returns_response() {
        let brain = MockBrain::new();
        let tool = Arc::new(MockTool::new("doc_rag").with_output(ToolOutput::Chunks {
            chunks: vec![
                make_chunk("1", "Title One", Some("https://example.com/1")),
                make_chunk("2", "Title Two", Some("https://example.com/2")),
            ],
            provider: "rag".into(),
        })) as Arc<dyn Tool>;

        let (pipeline, _rx) = make_pipeline(brain, vec![tool]);
        let req = make_request();
        let response = pipeline
            .search(&req, &TenantContext::public(), None)
            .await
            .unwrap();

        assert_eq!(response.request_id, "req-1");
        assert_eq!(response.session_id, "sess-1");

        // Should have LlmAnswer, Sources, and Suggestions components
        assert_eq!(response.components.len(), 3);
        let answer = response.components.iter().find_map(|c| {
            if let Component::LlmAnswer(a) = c {
                Some(a.answer.clone())
            } else {
                None
            }
        });
        assert_eq!(answer, Some("Mock answer".into()));

        // Metadata checks
        assert_eq!(response.metadata.locale, Locale::En);
        assert_eq!(response.metadata.intent, Intent::Factual);
        assert_eq!(response.metadata.retrieval_count, 2);
        assert_eq!(
            response.metadata.detected_locale_source,
            DetectedLocaleSource::LlmDetected
        );
    }

    #[tokio::test]
    async fn pipeline_search_with_empty_tools() {
        let brain = MockBrain::new();
        // Tool exists but won't fire
        let tool = Arc::new(MockTool::new("doc_rag").with_fire(false)) as Arc<dyn Tool>;

        let (pipeline, _rx) = make_pipeline(brain, vec![tool]);
        let req = make_request();
        let response = pipeline
            .search(&req, &TenantContext::public(), None)
            .await
            .unwrap();

        // Still returns a valid response, just no retrieval context
        assert_eq!(response.metadata.retrieval_count, 0);
        let answer = response.components.iter().find_map(|c| {
            if let Component::LlmAnswer(a) = c {
                Some(a.answer.clone())
            } else {
                None
            }
        });
        assert_eq!(answer, Some("Mock answer".into()));
    }

    #[tokio::test]
    async fn pipeline_search_passes_locale_from_brain() {
        let brain = MockBrain::new().with_translate_locale(Locale::Ja);
        let (pipeline, _rx) = make_pipeline(brain, vec![]);
        let req = make_request();
        let response = pipeline
            .search(&req, &TenantContext::public(), None)
            .await
            .unwrap();

        assert_eq!(response.metadata.locale, Locale::Ja);
        assert_eq!(
            response.metadata.detected_locale_source,
            DetectedLocaleSource::LlmDetected
        );
    }

    #[tokio::test]
    async fn pipeline_handles_brain_classify_error() {
        let brain = MockBrain::new().with_intent_fail();
        let (pipeline, _rx) = make_pipeline(brain, vec![]);
        let req = make_request();
        let response = pipeline
            .search(&req, &TenantContext::public(), None)
            .await
            .unwrap();

        // Falls back to Unknown
        assert_eq!(response.metadata.intent, Intent::Unknown);
    }

    #[tokio::test]
    async fn pipeline_handles_brain_translate_error() {
        let brain = MockBrain::new().with_translate_fail();
        let (pipeline, _rx) = make_pipeline(brain, vec![]);
        let req = make_request();
        let response = pipeline
            .search(&req, &TenantContext::public(), None)
            .await
            .unwrap();

        // Falls back to Locale::En with FallbackEn source
        assert_eq!(response.metadata.locale, Locale::En);
        assert_eq!(
            response.metadata.detected_locale_source,
            DetectedLocaleSource::FallbackEn
        );
        // Original query used as-is (no translation)
        assert!(response.metadata.translated_query.is_none());
    }

    #[tokio::test]
    async fn pipeline_records_conversation() {
        let brain = MockBrain::new();
        let (pipeline, mut rx) = make_pipeline(brain, vec![]);
        let req = make_request();
        let _response = pipeline
            .search(&req, &TenantContext::public(), None)
            .await
            .unwrap();

        // The conversation should be queued
        let record = rx.try_recv().unwrap();
        assert_eq!(record.query, "test query");
        assert_eq!(record.response_text, "Mock answer");
        assert_eq!(record.session_id, "sess-1");
        assert_eq!(record.request_id, "req-1");
        assert_eq!(record.locale, Locale::En);
        assert_eq!(record.intent, Intent::Factual);
    }

    #[tokio::test]
    async fn pipeline_appends_to_session_history() {
        let brain = MockBrain::new();
        let (pipeline, _rx) = make_pipeline(brain, vec![]);
        let req = make_request();
        let _response = pipeline
            .search(&req, &TenantContext::public(), None)
            .await
            .unwrap();

        // History should now have one turn
        let history = pipeline.history_store.snapshot_for_llm("sess-1");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].user, "test query");
        assert_eq!(history[0].assistant, "Mock answer");
    }

    #[tokio::test]
    async fn pipeline_suggestion_fallback_to_titles() {
        // Brain returns fewer suggestions than needed -> fallback to chunk titles
        let brain = MockBrain::new().with_suggestions(vec!["only one".into()]);
        let tool = Arc::new(MockTool::new("doc_rag").with_output(ToolOutput::Chunks {
            chunks: vec![
                make_chunk("1", "Chunk Title A", Some("https://a.com")),
                make_chunk("2", "Chunk Title B", Some("https://b.com")),
                make_chunk("3", "Chunk Title C", Some("https://c.com")),
            ],
            provider: "rag".into(),
        })) as Arc<dyn Tool>;

        let (pipeline, _rx) = make_pipeline(brain, vec![tool]);
        let req = make_request();
        let response = pipeline
            .search(&req, &TenantContext::public(), None)
            .await
            .unwrap();

        // Should fall back to chunk titles since suggest returned < suggestion_count
        let suggestions = response.components.iter().find_map(|c| {
            if let Component::Suggestions(s) = c {
                Some(s.suggestions.clone())
            } else {
                None
            }
        });
        let suggestions = suggestions.unwrap();
        assert_eq!(suggestions.len(), 3);
        assert_eq!(suggestions[0], "Chunk Title A");
        assert_eq!(suggestions[1], "Chunk Title B");
        assert_eq!(suggestions[2], "Chunk Title C");
    }

    #[tokio::test]
    async fn pipeline_search_stream_emits_start_and_chunks() {
        use futures::StreamExt;

        let brain = MockBrain::new();
        let tool = Arc::new(MockTool::new("doc_rag").with_output(ToolOutput::Chunks {
            chunks: vec![make_chunk("1", "Title", Some("https://example.com"))],
            provider: "rag".into(),
        })) as Arc<dyn Tool>;

        let (pipeline, _rx) = make_pipeline(brain, vec![tool]);
        let req = SearchRequest {
            query: "stream test".into(),
            session_id: "sess-stream".into(),
            request_id: "req-stream".into(),
            streaming: true,
            top_k: 5,
        };

        let output = pipeline
            .search_stream(&req, &TenantContext::public(), None)
            .await
            .unwrap();

        // Verify start_metadata
        assert_eq!(output.start_metadata.request_id, "req-stream");
        assert_eq!(output.start_metadata.session_id, "sess-stream");
        assert_eq!(output.start_metadata.locale, Locale::En);
        assert_eq!(output.start_metadata.intent, Intent::Factual);
        assert_eq!(output.start_metadata.retrieval_count, 1);

        // Drain stream and verify chunks
        let chunks: Vec<_> = output.stream.collect::<Vec<_>>().await;
        assert_eq!(chunks.len(), 1);
        let chunk = chunks[0].as_ref().unwrap();
        assert_eq!(chunk.delta, "Mock answer");
        assert!(chunk.finished);

        // Verify context
        assert_eq!(output.context.request_id, "req-stream");
        assert_eq!(output.context.locale, Locale::En);
    }

    #[tokio::test]
    async fn pipeline_complete_stream_merges_sources() {
        let brain = MockBrain::new();
        let (pipeline, _rx) = make_pipeline(brain, vec![]);

        let ctx = StreamContext {
            sources: vec![LlmSource {
                title: "Internal Source".into(),
                url: "https://internal.com".into(),
                snippet: None,
            }],
            llm_model: "gemini".into(),
            start_instant: Instant::now(),
            grounding: GroundingInfo::default(),
            request_id: "req-cs".into(),
            session_id: "sess-cs".into(),
            history_key: "sess-cs".into(),
            query: "test".into(),
            locale: Locale::En,
            intent: Intent::Factual,
            tenant: TenantContext::public(),
            _cancel_guard: CancelGuard::new(CancellationToken::new()),
        };

        let grounding_sources = vec![LlmSource {
            title: "Grounding Source".into(),
            url: "https://grounding.com".into(),
            snippet: Some("snippet".into()),
        }];

        let done = pipeline
            .complete_stream(ctx, "accumulated answer", grounding_sources)
            .await;

        // Grounding sources come first, then internal sources
        assert_eq!(done.sources.len(), 2);
        assert_eq!(done.sources[0].title, "Grounding Source");
        assert_eq!(done.sources[1].title, "Internal Source");

        // Gemini grounding flag should be set
        assert!(done.grounding.gemini_grounding_used);

        // Suggestions from the brain
        assert_eq!(done.suggestions.len(), 3);
    }

    #[tokio::test]
    async fn pipeline_complete_stream_dedupes_sources_by_url() {
        let brain = MockBrain::new();
        let (pipeline, _rx) = make_pipeline(brain, vec![]);

        let ctx = StreamContext {
            sources: vec![LlmSource {
                title: "Internal Title".into(),
                url: "https://same-url.com".into(),
                snippet: None,
            }],
            llm_model: "gemini".into(),
            start_instant: Instant::now(),
            grounding: GroundingInfo::default(),
            request_id: "req-dd".into(),
            session_id: "sess-dd".into(),
            history_key: "sess-dd".into(),
            query: "test".into(),
            locale: Locale::En,
            intent: Intent::Factual,
            tenant: TenantContext::public(),
            _cancel_guard: CancelGuard::new(CancellationToken::new()),
        };

        // Same URL as internal source -- grounding should win
        let grounding_sources = vec![LlmSource {
            title: "Grounding Title".into(),
            url: "https://same-url.com".into(),
            snippet: Some("snippet".into()),
        }];

        let done = pipeline
            .complete_stream(ctx, "answer", grounding_sources)
            .await;

        // Only one source after dedup -- grounding wins because it's first
        assert_eq!(done.sources.len(), 1);
        assert_eq!(done.sources[0].title, "Grounding Title");
    }

    #[tokio::test]
    async fn pipeline_complete_stream_no_grounding() {
        let brain = MockBrain::new();
        let (pipeline, _rx) = make_pipeline(brain, vec![]);

        let ctx = StreamContext {
            sources: vec![LlmSource {
                title: "Internal".into(),
                url: "https://internal.com".into(),
                snippet: None,
            }],
            llm_model: "gemini".into(),
            start_instant: Instant::now(),
            grounding: GroundingInfo::default(),
            request_id: "req-ng".into(),
            session_id: "sess-ng".into(),
            history_key: "sess-ng".into(),
            query: "test".into(),
            locale: Locale::En,
            intent: Intent::Factual,
            tenant: TenantContext::public(),
            _cancel_guard: CancelGuard::new(CancellationToken::new()),
        };

        let done = pipeline.complete_stream(ctx, "answer", vec![]).await;

        assert_eq!(done.sources.len(), 1);
        assert!(!done.grounding.gemini_grounding_used);
    }

    #[tokio::test]
    async fn pipeline_complete_stream_records_conversation() {
        let brain = MockBrain::new();
        let (pipeline, mut rx) = make_pipeline(brain, vec![]);

        let ctx = StreamContext {
            sources: vec![],
            llm_model: "gemini".into(),
            start_instant: Instant::now(),
            grounding: GroundingInfo::default(),
            request_id: "req-cr".into(),
            session_id: "sess-cr".into(),
            history_key: "sess-cr".into(),
            query: "streamed query".into(),
            locale: Locale::Ja,
            intent: Intent::Navigational,
            tenant: TenantContext::public(),
            _cancel_guard: CancelGuard::new(CancellationToken::new()),
        };

        let _done = pipeline
            .complete_stream(ctx, "streamed answer", vec![])
            .await;

        let record = rx.try_recv().unwrap();
        assert_eq!(record.query, "streamed query");
        assert_eq!(record.response_text, "streamed answer");
        assert_eq!(record.locale, Locale::Ja);
        assert_eq!(record.intent, Intent::Navigational);
    }

    #[tokio::test]
    async fn pipeline_complete_stream_appends_history() {
        let brain = MockBrain::new();
        let (pipeline, _rx) = make_pipeline(brain, vec![]);

        let ctx = StreamContext {
            sources: vec![],
            llm_model: "gemini".into(),
            start_instant: Instant::now(),
            grounding: GroundingInfo::default(),
            request_id: "req-h".into(),
            session_id: "sess-h".into(),
            history_key: "history-key".into(),
            query: "history query".into(),
            locale: Locale::En,
            intent: Intent::Factual,
            tenant: TenantContext::public(),
            _cancel_guard: CancelGuard::new(CancellationToken::new()),
        };

        let _done = pipeline
            .complete_stream(ctx, "history answer", vec![])
            .await;

        let history = pipeline.history_store.snapshot_for_llm("history-key");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].user, "history query");
        assert_eq!(history[0].assistant, "history answer");
    }

    #[tokio::test]
    async fn pipeline_empty_answer_skips_history() {
        let brain = MockBrain::new();
        let (pipeline, _rx) = make_pipeline(brain, vec![]);

        let ctx = StreamContext {
            sources: vec![],
            llm_model: "gemini".into(),
            start_instant: Instant::now(),
            grounding: GroundingInfo::default(),
            request_id: "req-e".into(),
            session_id: "sess-e".into(),
            history_key: "empty-key".into(),
            query: "q".into(),
            locale: Locale::En,
            intent: Intent::Factual,
            tenant: TenantContext::public(),
            _cancel_guard: CancelGuard::new(CancellationToken::new()),
        };

        let _done = pipeline.complete_stream(ctx, "", vec![]).await;

        let history = pipeline.history_store.snapshot_for_llm("empty-key");
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn pipeline_device_session_id_used_for_history() {
        let brain = MockBrain::new();
        let (pipeline, _rx) = make_pipeline(brain, vec![]);
        let req = make_request();

        // Pass a device session id different from the body session_id
        let _response = pipeline
            .search(&req, &TenantContext::public(), Some("device-123"))
            .await
            .unwrap();

        // History should be keyed by device session id
        let device_history = pipeline.history_store.snapshot_for_llm("device-123");
        assert_eq!(device_history.len(), 1);

        // Body session_id should have no history
        let body_history = pipeline.history_store.snapshot_for_llm("sess-1");
        assert!(body_history.is_empty());
    }

    #[test]
    fn llm_model_name_reads_from_brain_default() {
        // MockBrain uses Brain trait default model_name() -> "unknown".
        // Real GeminiBrain provides the configured model name.
        let brain = MockBrain::new();
        let (pipeline, _rx) = make_pipeline(brain, vec![]);
        assert_eq!(pipeline.llm_model_name(), "unknown");
    }
}
