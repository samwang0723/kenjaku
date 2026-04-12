use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use futures::Stream;
use tracing::warn;

use kenjaku_core::config::WebSearchConfig;
use kenjaku_core::error::Result;
use kenjaku_core::traits::intent::IntentClassifier;
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::traits::retriever::Retriever;
use kenjaku_core::traits::tool::Tool;
use kenjaku_core::traits::web_search::WebSearchProvider;
use kenjaku_core::types::intent::Intent;
use kenjaku_core::types::locale::{DetectedLocale, Locale};
use kenjaku_core::types::search::{
    DetectedLocaleSource, GroundingInfo, LlmSource, SearchRequest, SearchResponse, StreamChunk,
    StreamDoneMetadata, StreamStartMetadata, TranslationResult,
};
use kenjaku_core::types::tool::ToolConfig;

use crate::component::ComponentService;
use crate::conversation::ConversationService;
use crate::harness::SearchOrchestrator;
use crate::history::SessionHistoryStore;
use crate::locale_memory::LocaleMemory;
use crate::tools::{BraveWebTool, DocRagTool};
use crate::translation::TranslationService;
use crate::trending::TrendingService;

/// Orchestrates the full RAG search pipeline.
///
/// Public interface consumed by `kenjaku-api` handlers. Internally
/// delegates to `SearchOrchestrator` which routes through the `Tool`
/// trait for retrieval and web search.
pub struct SearchService {
    orchestrator: SearchOrchestrator,
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
        // Build tools from the existing dependencies.
        let doc_rag: Arc<dyn Tool> = Arc::new(DocRagTool::new(
            retriever,
            collection_name.clone(),
            ToolConfig::default(),
        ));

        let brave_web: Arc<dyn Tool> = Arc::new(BraveWebTool::new(
            web_search,
            ToolConfig {
                enabled: web_search_config.enabled,
                rollout_pct: None,
            },
            web_search_config.trigger_patterns.clone(),
            web_search_config.fallback_min_chunks,
        ));

        let tools: Vec<Arc<dyn Tool>> = vec![doc_rag, brave_web];

        let orchestrator = SearchOrchestrator::new(
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
            &web_search_config,
            collection_name,
            suggestion_count,
        );

        Self { orchestrator }
    }

    /// Execute a non-streaming search.
    pub async fn search(
        &self,
        req: &SearchRequest,
        device_session_id: Option<&str>,
    ) -> Result<SearchResponse> {
        self.orchestrator.search(req, device_session_id).await
    }

    /// Execute a streaming search.
    ///
    /// Returns a `SearchStreamOutput` containing:
    /// - `start_metadata` -- everything we know BEFORE the LLM stream begins
    /// - `stream` -- the token delta stream
    /// - `context` -- bookkeeping for `complete_stream`
    pub async fn search_stream(
        &self,
        req: &SearchRequest,
        device_session_id: Option<&str>,
    ) -> Result<SearchStreamOutput> {
        self.orchestrator.search_stream(req, device_session_id).await
    }

    /// Called by the handler after the token stream finishes. Produces the
    /// final `done` metadata and queues the conversation for async persistence.
    pub async fn complete_stream(
        &self,
        ctx: StreamContext,
        accumulated_answer: &str,
        grounding_sources: Vec<LlmSource>,
    ) -> StreamDoneMetadata {
        self.orchestrator
            .complete_stream(ctx, accumulated_answer, grounding_sources)
            .await
    }
}

/// Reconcile the translator's `Result<TranslationResult>` into the three
/// values the search pipeline needs: the English-normalized search query,
/// the resolved `Locale` to answer in, and the provenance of that locale.
///
/// Failure modes:
/// - Translator error -> `(raw_query, Locale::En, FallbackEn)` -- we have
///   no normalized form to fall back to, so the raw query goes to
///   retrieval as-is.
/// - Unsupported BCP-47 tag (e.g. `pt`, `it`) -> `(tr.normalized,
///   Locale::En, FallbackEn)`. We keep the translator's English-normalized
///   form because it was successfully produced and is better for
///   retrieval than the raw non-English input; only the *answer language*
///   falls back to English.
///
/// Either way the search hot path never blocks.
pub(crate) fn resolve_translation(
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

/// Bookkeeping passed from `search_stream` to `complete_stream` -- kept
/// separate from the Stream so the handler can move it into the completion
/// call after dropping the stream.
pub struct StreamContext {
    pub sources: Vec<LlmSource>,
    pub llm_model: String,
    pub start_instant: Instant,
    /// Web tier provenance captured during the preamble (before the
    /// LLM stream opens). `complete_stream` may also flip
    /// `gemini_grounding_used` if Gemini attached grounding metadata
    /// to the stream.
    pub grounding: GroundingInfo,
    pub request_id: String,
    pub session_id: String,
    /// Stable device/session key used for in-memory history and locale
    /// memory. Prefer `X-Session-Id` header; falls back to body session_id.
    pub history_key: String,
    pub query: String,
    pub locale: kenjaku_core::types::locale::Locale,
    pub intent: Intent,
}
