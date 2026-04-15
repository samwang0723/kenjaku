use std::sync::Arc;

use tokio::signal;
use tracing::info;

use kenjaku_core::config::load_config;
use kenjaku_core::traits::brain::Brain;
use kenjaku_core::traits::tool::Tool;
use kenjaku_core::types::tool::ToolConfig;
use kenjaku_infra::clustering::LinfaClusterer;
use kenjaku_infra::embedding::create_embedding_provider;
use kenjaku_infra::llm::GeminiProvider;
use kenjaku_infra::postgres::{
    ConversationRepository, DefaultSuggestionsRepository, FeedbackRepository,
    RefreshBatchesRepository, TrendingRepository, create_pool, run_migrations,
};
use kenjaku_infra::qdrant::QdrantClient;
use kenjaku_infra::redis::{LocaleMemoryRedis, RedisClient};
use kenjaku_infra::telemetry::init_telemetry;
use kenjaku_infra::title_resolver::TitleResolver;
use kenjaku_infra::web_search::BraveSearchProvider;

use kenjaku_service::autocomplete::AutocompleteService;
use kenjaku_service::brain::GeminiBrain;
use kenjaku_service::component::ComponentService;
use kenjaku_service::conversation::ConversationService;
use kenjaku_service::feedback::FeedbackService;
use kenjaku_service::intent::LlmIntentClassifier;
use kenjaku_service::locale_memory::LocaleMemory;
use kenjaku_service::pipelines::SinglePassPipeline;
use kenjaku_service::refresh_worker::SuggestionRefreshWorker;
use kenjaku_service::retriever::HybridRetriever;
use kenjaku_service::search::SearchService;
use kenjaku_service::suggestion::{ServiceRng, SuggestionService};
use kenjaku_service::tools::{BraveWebTool, DocRagTool};
use kenjaku_service::trending::TrendingService;
use kenjaku_service::worker::TrendingFlushWorker;

use kenjaku_api::AppState;
use kenjaku_api::router::build_router;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load configuration and validate secrets are present
    let config = load_config()?;
    config.validate_secrets()?;

    // Initialize telemetry
    let _tracer_provider = init_telemetry(&config.telemetry)?;

    info!(
        host = %config.server.host,
        port = config.server.port,
        "Starting Kenjaku server"
    );

    // Initialize infrastructure
    let pg_pool = create_pool(&config.postgres).await?;
    run_migrations(&pg_pool).await?;

    let qdrant = QdrantClient::new(config.qdrant.clone()).await?;
    qdrant.ensure_collection().await?;

    let redis = RedisClient::new(&config.redis).await?;

    // Create providers
    let embedding_provider = Arc::from(create_embedding_provider(config.embedding.clone())?);
    // Attach Gemini's built-in `google_search` tool only when no
    // external `WebSearchProvider` is wired in. If `web_search.enabled`
    // is true, Brave (or whichever provider) pre-injects fresh web
    // results as synthetic `[Source N]` chunks — google_search becomes
    // redundant and we skip it to save tokens and latency. When
    // disabled, google_search is the fallback source of live facts.
    let use_google_search_tool = !config.web_search.enabled;
    let llm_provider = Arc::new(GeminiProvider::new(
        config.llm.clone(),
        use_google_search_tool,
    ));

    // Create repositories
    let feedback_repo = FeedbackRepository::new(pg_pool.clone());
    let trending_repo = TrendingRepository::new(pg_pool.clone());
    let conversation_repo = ConversationRepository::new(pg_pool.clone());
    let default_suggestions_repo = DefaultSuggestionsRepository::new(pg_pool.clone());
    let refresh_batches_repo = RefreshBatchesRepository::new(pg_pool.clone());

    // Create services
    let retriever = Arc::new(HybridRetriever::new(
        qdrant.clone(),
        embedding_provider,
        config.search.semantic_weight,
        config.search.bm25_weight,
        config.search.over_retrieve_factor,
    ));

    let intent_classifier = Arc::new(LlmIntentClassifier::new(llm_provider.clone()));

    let component_service = ComponentService::new(config.search.component_layout.clone());
    let trending_service = TrendingService::new(
        redis.clone(),
        trending_repo.clone(),
        config.trending.clone(),
    );
    let autocomplete_service = AutocompleteService::new(
        trending_repo.clone(),
        qdrant.clone(),
        config.trending.crowd_sourcing_min_count,
    );
    let feedback_service = FeedbackService::new(feedback_repo);

    // Conversation service with async flush worker (buffer 1024 records)
    let (conversation_service, conversation_worker) =
        ConversationService::new(conversation_repo, 1024);

    // Title resolver: resolves Gemini google_search redirect URLs into
    // real page titles, with Redis-backed caching.
    let title_resolver = Arc::new(TitleResolver::new(Some(redis.connection_manager())));

    // LocaleMemory: per-session sticky locale stored in Redis. Recorded
    // by SearchService on every query, read by suggestion read paths
    // (see TODO at the top of this fn re: SessionLocaleLookup adapter).
    let locale_memory_redis = LocaleMemoryRedis::new(redis.connection_manager());
    let locale_memory = Arc::new(LocaleMemory::new(
        locale_memory_redis,
        config.locale_memory.clone(),
    ));

    // SuggestionService: blends crowdsourced trending with materialized
    // default suggestions. Read-only, hot-path safe. Wired into AppState
    // and consumed by the `top_searches` / `autocomplete` handlers.
    let suggestion_service = Arc::new(SuggestionService::new(
        trending_repo.clone(),
        default_suggestions_repo.clone(),
        config.default_suggestions.pool_cap,
        config.trending.crowd_sourcing_min_count,
        Arc::new(ServiceRng::from_entropy()),
    ));

    // In-memory per-session conversation history — supplies follow-up
    // context to the LLM call. NOT a replacement for the durable
    // `conversations` table. Janitor evicts idle sessions so abandoned
    // clients don't leak memory.
    let history_store =
        kenjaku_service::history::SessionHistoryStore::new(config.search.history.clone());
    history_store.clone().spawn_janitor();

    // Build the Brain facade that wraps LLM + intent classifier.
    let brain: Arc<dyn Brain> = Arc::new(GeminiBrain::new(llm_provider.clone(), intent_classifier));

    // Build the Tool list — DocRag (tier 1) then BraveWeb (tier 2).
    let doc_rag: Arc<dyn Tool> = Arc::new(DocRagTool::new(
        retriever,
        config.qdrant.collection_name.clone(),
        ToolConfig::default(),
    ));

    // Web search provider — replaces Gemini's non-functional built-in
    // `google_search` tool. Constructed only when enabled + configured
    // with an API key; otherwise the BraveWebTool wraps None and never
    // fires.
    let web_search_provider = if config.web_search.enabled && !config.web_search.api_key.is_empty()
    {
        match config.web_search.provider.as_str() {
            "brave" => match BraveSearchProvider::new(config.web_search.clone()) {
                Ok(p) => Some(Arc::new(p)),
                Err(e) => {
                    tracing::error!(error = %e, "Failed to init Brave provider; web tier disabled");
                    None
                }
            },
            other => {
                tracing::warn!(provider = %other, "unknown web_search.provider; web tier disabled");
                None
            }
        }
    } else {
        None
    };
    info!(
        enabled = config.web_search.enabled,
        configured = web_search_provider.is_some(),
        "Web search tier"
    );

    let brave_web: Arc<dyn Tool> = Arc::new(BraveWebTool::new(
        web_search_provider.map(|p| p as _),
        ToolConfig {
            enabled: config.web_search.enabled,
            rollout_pct: None,
        },
        config.web_search.trigger_patterns.clone(),
        config.web_search.fallback_min_chunks,
        config.web_search.limit,
    ));

    let tools: Vec<Arc<dyn Tool>> = vec![doc_rag, brave_web];

    // Build the search pipeline (single-pass today; a future `AgenticPipeline`
    // or `CachedPipeline` would be selected here based on config).
    let pipeline = Arc::new(SinglePassPipeline::new(
        brain,
        component_service,
        trending_service.clone(),
        conversation_service,
        Some(title_resolver),
        locale_memory.clone(),
        history_store,
        tools,
        &config.web_search,
        config.qdrant.collection_name.clone(),
        config.search.suggestion_count,
        use_google_search_tool,
    ));

    let search_service = SearchService::new(pipeline);

    // Spawn background workers
    let flush_worker = TrendingFlushWorker::new(
        redis.clone(),
        trending_repo.clone(),
        config.trending.clone(),
    );
    tokio::spawn(flush_worker.run());
    tokio::spawn(conversation_worker.run());

    // SuggestionRefreshWorker: daily 03:00 UTC rebuild of default
    // suggestion pool. Gated by `default_suggestions.enabled`. Same
    // fire-and-spawn pattern as TrendingFlushWorker — graceful
    // shutdown happens implicitly when the runtime drops the task at
    // server exit.
    if config.default_suggestions.enabled {
        let refresh_worker = SuggestionRefreshWorker::new(
            pg_pool.clone(),
            Arc::new(qdrant.clone()),
            Arc::new(LinfaClusterer::new()),
            llm_provider.clone(),
            default_suggestions_repo.clone(),
            refresh_batches_repo.clone(),
            config.default_suggestions.clone(),
            config.qdrant.collection_name.clone(),
        );
        tokio::spawn(refresh_worker.run_scheduled());
    }

    // Adapter so the api crate's `ResolvedLocale` extractor can resolve
    // session-stickied locales without taking a direct dependency on the
    // service-layer `LocaleMemory`. Injected as a request `Extension` by
    // `build_router` below.
    let locale_lookup: Arc<dyn kenjaku_api::extractors::SessionLocaleLookup> =
        Arc::new(LocaleMemoryAdapter(locale_memory.clone()));

    // Build app state
    let state = Arc::new(AppState {
        search_service,
        trending_service,
        autocomplete_service,
        suggestion_service,
        feedback_service,
        qdrant,
        redis,
        pg_pool,
    });

    // Build router
    let app = build_router(state, locale_lookup, &config.server);

    // Bind and serve
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(addr = %addr, "Server listening");

    // into_make_service_with_connect_info exposes the peer SocketAddr to handlers
    // and middleware (e.g. the rate limiter's SmartIpKeyExtractor).
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    info!("Server shut down gracefully");
    Ok(())
}

/// Newtype wrapping the service-layer `LocaleMemory` so it can be exposed
/// to the api crate's `SessionLocaleLookup` trait without leaking the
/// concrete type across the layer boundary.
struct LocaleMemoryAdapter(Arc<LocaleMemory>);

#[async_trait::async_trait]
impl kenjaku_api::extractors::SessionLocaleLookup for LocaleMemoryAdapter {
    async fn lookup(&self, session_id: &str) -> Option<kenjaku_core::types::locale::Locale> {
        self.0.lookup(session_id).await
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received");
}
