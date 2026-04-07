use std::sync::Arc;

use tokio::signal;
use tracing::info;

use kenjaku_core::config::load_config;
use kenjaku_infra::embedding::create_embedding_provider;
use kenjaku_infra::llm::GeminiProvider;
use kenjaku_infra::postgres::{
    ConversationRepository, FeedbackRepository, TrendingRepository, create_pool, run_migrations,
};
use kenjaku_infra::qdrant::QdrantClient;
use kenjaku_infra::redis::RedisClient;
use kenjaku_infra::telemetry::init_telemetry;

use kenjaku_service::autocomplete::AutocompleteService;
use kenjaku_service::component::ComponentService;
use kenjaku_service::conversation::ConversationService;
use kenjaku_service::feedback::FeedbackService;
use kenjaku_service::intent::LlmIntentClassifier;
use kenjaku_service::retriever::HybridRetriever;
use kenjaku_service::search::SearchService;
use kenjaku_service::translation::TranslationService;
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
    let llm_provider = Arc::new(GeminiProvider::new(config.llm.clone()));

    // Create repositories
    let feedback_repo = FeedbackRepository::new(pg_pool.clone());
    let trending_repo = TrendingRepository::new(pg_pool.clone());
    let conversation_repo = ConversationRepository::new(pg_pool.clone());

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
    let translation_service = TranslationService::new(llm_provider.clone());
    let trending_service = TrendingService::new(
        redis.clone(),
        trending_repo.clone(),
        config.trending.clone(),
    );
    let autocomplete_service = AutocompleteService::new(trending_repo.clone(), qdrant.clone());
    let feedback_service = FeedbackService::new(feedback_repo);

    // Conversation service with async flush worker (buffer 1024 records)
    let (conversation_service, conversation_worker) =
        ConversationService::new(conversation_repo, 1024);

    let search_service = SearchService::new(
        retriever,
        llm_provider,
        intent_classifier,
        component_service,
        translation_service,
        trending_service.clone(),
        conversation_service,
        config.qdrant.collection_name.clone(),
        config.search.suggestion_count,
    );

    // Spawn background workers
    let flush_worker =
        TrendingFlushWorker::new(redis.clone(), trending_repo, config.trending.clone());
    tokio::spawn(flush_worker.run());
    tokio::spawn(conversation_worker.run());

    // Build app state
    let state = Arc::new(AppState {
        search_service,
        trending_service,
        autocomplete_service,
        feedback_service,
        qdrant,
        redis,
        pg_pool,
    });

    // Build router
    let app = build_router(state);

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
