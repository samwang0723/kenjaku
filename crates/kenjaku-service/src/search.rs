use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use futures::Stream;
use tracing::{info, instrument, warn};

use kenjaku_core::config::WebSearchConfig;
use kenjaku_core::error::Result;
use kenjaku_core::traits::intent::IntentClassifier;
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::traits::retriever::Retriever;
use kenjaku_core::traits::web_search::WebSearchProvider;
use kenjaku_core::types::component::SuggestionSource;
use kenjaku_core::types::conversation::{ConversationTurn, CreateConversation};
use kenjaku_core::types::intent::Intent;
use kenjaku_core::types::locale::{DetectedLocale, Locale};
use kenjaku_core::types::search::{
    DetectedLocaleSource, LlmSource, RetrievalMethod, RetrievedChunk, SearchMetadata,
    SearchRequest, SearchResponse, StreamChunk, StreamDoneMetadata, StreamStartMetadata,
    TranslationResult,
};
use regex::Regex;

use crate::component::ComponentService;
use crate::conversation::ConversationService;
use crate::history::SessionHistoryStore;
use crate::locale_memory::LocaleMemory;
use crate::quality::prettify_title;
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
    title_resolver: Option<Arc<kenjaku_infra::title_resolver::TitleResolver>>,
    locale_memory: Arc<LocaleMemory>,
    history_store: SessionHistoryStore,
    web_search: Option<Arc<dyn WebSearchProvider>>,
    web_search_config: WebSearchConfig,
    web_search_triggers: Vec<Regex>,
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
        title_resolver: Option<Arc<kenjaku_infra::title_resolver::TitleResolver>>,
        locale_memory: Arc<LocaleMemory>,
        history_store: SessionHistoryStore,
        web_search: Option<Arc<dyn WebSearchProvider>>,
        web_search_config: WebSearchConfig,
        collection_name: String,
        suggestion_count: usize,
    ) -> Self {
        // Pre-compile trigger regexes once. Bad patterns are dropped with
        // a warning so a single misconfigured entry doesn't disable the
        // whole web tier.
        let web_search_triggers = web_search_config
            .trigger_patterns
            .iter()
            .filter_map(|p| match Regex::new(p) {
                Ok(r) => Some(r),
                Err(e) => {
                    warn!(pattern = %p, error = %e, "dropping invalid web_search trigger pattern");
                    None
                }
            })
            .collect();

        Self {
            retriever,
            llm,
            intent_classifier,
            component_service,
            translation_service,
            trending_service,
            conversation_service,
            title_resolver,
            locale_memory,
            history_store,
            web_search,
            web_search_config,
            web_search_triggers,
            collection_name,
            suggestion_count,
        }
    }

    /// Decide whether to hit the web tier for this query. Trigger when:
    /// (a) web search is enabled and configured,
    /// (b) the query matches any configured pattern, OR
    /// (c) internal retrieval returned fewer than `fallback_min_chunks`.
    fn should_web_search(&self, search_query: &str, internal_chunk_count: usize) -> bool {
        if !self.web_search_config.enabled || self.web_search.is_none() {
            return false;
        }
        if self
            .web_search_triggers
            .iter()
            .any(|r| r.is_match(search_query))
        {
            return true;
        }
        internal_chunk_count < self.web_search_config.fallback_min_chunks
    }

    /// Fetch top-N web results and convert them into synthetic
    /// `RetrievedChunk`s the LLM can cite via `[Source N]`. Always
    /// fallible-safe: on error we log and return an empty vec so the
    /// search hot path never blocks.
    async fn fetch_web_chunks(&self, query: &str) -> Vec<RetrievedChunk> {
        let Some(provider) = self.web_search.as_ref() else {
            return Vec::new();
        };
        match provider.search(query, self.web_search_config.limit).await {
            Ok(results) => {
                info!(
                    count = results.len(),
                    query = %query,
                    "web search returned results"
                );
                results
                    .into_iter()
                    .enumerate()
                    .map(|(i, r)| RetrievedChunk {
                        doc_id: format!("web-{i}"),
                        chunk_id: format!("web-{i}"),
                        title: r.title,
                        original_content: r.snippet.clone(),
                        contextualized_content: r.snippet,
                        source_url: Some(r.url),
                        score: 0.0,
                        retrieval_method: RetrievalMethod::Web,
                    })
                    .collect()
            }
            Err(e) => {
                warn!(error = %e, "web search failed; continuing with internal-only retrieval");
                Vec::new()
            }
        }
    }

    /// Execute a non-streaming search.
    #[instrument(skip(self), fields(
        request_id = %req.request_id,
    ))]
    pub async fn search(
        &self,
        req: &SearchRequest,
        device_session_id: Option<&str>,
    ) -> Result<SearchResponse> {
        let start = Instant::now();

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
        //
        // PR9 #8: prefer the device id from `X-Session-Id` because that's
        // what the read-path extractor keys on. Falls back to the body
        // session_id if the client did not send the header.
        {
            let lm = self.locale_memory.clone();
            let sid = device_session_id
                .map(str::to_owned)
                .unwrap_or_else(|| req.session_id.clone());
            tokio::spawn(async move {
                lm.record(&sid, detected_locale).await;
            });
        }

        // Step 3: Retrieve with hybrid search (uses the English-normalized
        // query, regardless of detected locale).
        let mut chunks = self
            .retriever
            .retrieve(&search_query, &self.collection_name, req.top_k)
            .await?;

        // Step 3b: Web tier. For time-sensitive queries or when internal
        // retrieval is sparse, augment with live web results via the
        // configured provider (Brave by default). Web chunks are
        // appended so the model sees them as `[Source N]` entries
        // alongside internal corpus chunks.
        if self.should_web_search(&search_query, chunks.len()) {
            let web_chunks = self.fetch_web_chunks(&search_query).await;
            chunks.extend(web_chunks);
        }

        // Session-scoped conversation history for follow-up context.
        // Keyed by device_session_id (X-Session-Id header) with fallback
        // to body session_id — same keying as LocaleMemory.
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
        // locale, not a client hint. The trending service applies a
        // gibberish guard and normalizes the stored form.
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
        // same device can see this exchange. Done AFTER the successful
        // response so failed calls don't pollute follow-up context.
        // We store the ORIGINAL user query (not the English-normalized
        // form) so model-side attention matches what the user actually
        // said, preserving tone and proper nouns.
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
    ))]
    pub async fn search_stream(
        &self,
        req: &SearchRequest,
        device_session_id: Option<&str>,
    ) -> Result<SearchStreamOutput> {
        let start = Instant::now();

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

        // Fire-and-forget: record detected locale for the session. PR9 #8:
        // prefer device id from `X-Session-Id` so the read path can hit
        // the same key.
        {
            let lm = self.locale_memory.clone();
            let sid = device_session_id
                .map(str::to_owned)
                .unwrap_or_else(|| req.session_id.clone());
            tokio::spawn(async move {
                lm.record(&sid, detected_locale).await;
            });
        }

        // Step 3: Retrieve
        let mut chunks = self
            .retriever
            .retrieve(&search_query, &self.collection_name, req.top_k)
            .await?;

        // Step 3b: Web tier (streaming path).
        if self.should_web_search(&search_query, chunks.len()) {
            let web_chunks = self.fetch_web_chunks(&search_query).await;
            chunks.extend(web_chunks);
        }

        // Sources are known at this point (from retrieved chunks). Dedupe by
        // URL so multiple chunks from the same document produce a single
        // source entry, and prettify slug-shaped titles for display.
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

        // Pull session history for follow-up context (streaming path).
        let history_key = device_session_id.unwrap_or(&req.session_id);
        let history = self.history_store.snapshot_for_llm(history_key);

        // Step 4: Open the LLM stream pinned to the detected locale.
        let stream = self
            .llm
            .generate_stream(&search_query, &chunks, &history, detected_locale)
            .await?;

        // Step 5: Record trending (fire-and-forget) under the DETECTED
        // locale. The trending service applies a gibberish guard and
        // normalizes the stored form.
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
                history_key: history_key.to_string(),
                query: req.query.clone(),
                locale: detected_locale,
                intent,
            },
        })
    }

    /// Called by the handler after the token stream finishes. Produces the final
    /// `done` metadata and queues the conversation for async persistence.
    ///
    /// `grounding_sources` is whatever the handler accumulated from
    /// `StreamChunk.grounding` while draining the stream — i.e. the
    /// google_search sources Gemini cited. They are resolved (in parallel,
    /// with caching) into real page titles via the optional `TitleResolver`,
    /// then deduped against the internal `ctx.sources` and appended.
    pub async fn complete_stream(
        &self,
        ctx: StreamContext,
        accumulated_answer: &str,
        grounding_sources: Vec<LlmSource>,
    ) -> StreamDoneMetadata {
        let suggestions = match self.llm.suggest(&ctx.query, accumulated_answer).await {
            Ok(s) if s.len() >= self.suggestion_count => s[..self.suggestion_count].to_vec(),
            Ok(s) => s,
            Err(_) => Vec::new(),
        };

        // Resolve grounding URLs in parallel (cache-backed) into real titles,
        // then merge with the internal-chunk sources, dedupe by URL.
        let resolved_grounding = if grounding_sources.is_empty() {
            Vec::new()
        } else if let Some(resolver) = self.title_resolver.as_ref() {
            resolver.resolve_batch(grounding_sources).await
        } else {
            grounding_sources
        };

        // Order: google_search grounding sources first (real-time, web-grounded
        // facts) followed by internal chunk sources (product knowledge base).
        // Deduped by URL — if a URL appears in both, the grounding entry wins
        // because it carries the resolved page title.
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

        // Append to in-memory session history (streaming path). Guard on
        // non-empty answer so cancelled/errored streams don't pollute.
        if !accumulated_answer.is_empty() {
            self.history_store.append(
                &ctx.history_key,
                ConversationTurn {
                    user: ctx.query.clone(),
                    assistant: accumulated_answer.to_string(),
                },
            );
        }

        StreamDoneMetadata {
            latency_ms,
            sources: merged_sources,
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

/// Reconcile the translator's `Result<TranslationResult>` into the three
/// values the search pipeline needs: the English-normalized search query,
/// the resolved `Locale` to answer in, and the provenance of that locale.
///
/// Failure modes:
/// - Translator error → `(raw_query, Locale::En, FallbackEn)` — we have
///   no normalized form to fall back to, so the raw query goes to
///   retrieval as-is.
/// - Unsupported BCP-47 tag (e.g. `pt`, `it`) → `(tr.normalized,
///   Locale::En, FallbackEn)`. We keep the translator's English-normalized
///   form because it was successfully produced and is better for
///   retrieval than the raw non-English input; only the *answer language*
///   falls back to English.
///
/// Either way the search hot path never blocks.
fn resolve_translation(
    raw_query: &str,
    result: Result<TranslationResult>,
) -> (String, Locale, DetectedLocaleSource) {
    match result {
        Ok(tr) => match tr.detected_locale {
            DetectedLocale::Supported(l) => (tr.normalized, l, DetectedLocaleSource::LlmDetected),
            DetectedLocale::Unsupported { tag } => {
                warn!(
                    detected_tag = %tag,
                    "Translator detected an unsupported locale; falling back to English"
                );
                (tr.normalized, Locale::En, DetectedLocaleSource::FallbackEn)
            }
        },
        Err(e) => {
            warn!(
                error = %e,
                "Translator failed; falling back to raw query + en"
            );
            (
                raw_query.to_string(),
                Locale::En,
                DetectedLocaleSource::FallbackEn,
            )
        }
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
    /// Stable device/session key used for in-memory history and locale
    /// memory. Prefer `X-Session-Id` header; falls back to body session_id.
    pub history_key: String,
    pub query: String,
    pub locale: kenjaku_core::types::locale::Locale,
    pub intent: Intent,
}
