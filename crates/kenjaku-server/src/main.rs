use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::signal;
use tracing::info;

use kenjaku_core::config::{TenancyConfig, load_config};
use kenjaku_core::error::{Error, Result as KenjakuResult};
use kenjaku_core::traits::brain::Brain;
use kenjaku_core::traits::classifier::Classifier;
use kenjaku_core::traits::collection::{CollectionResolver, PrefixCollectionResolver};
use kenjaku_core::traits::generator::Generator;
use kenjaku_core::traits::tool::Tool;
use kenjaku_core::traits::translator::Translator;
use kenjaku_core::types::tool::ToolConfig;
use kenjaku_infra::auth::JwtValidator;
use kenjaku_infra::clustering::LinfaClusterer;
use kenjaku_infra::embedding::create_embedding_provider;
use kenjaku_infra::llm::GeminiProvider;
use kenjaku_infra::postgres::{
    ConversationRepository, DefaultSuggestionsRepository, FeedbackRepository,
    RefreshBatchesRepository, TenantsCache, TrendingRepository, create_pool, run_migrations,
};
use kenjaku_infra::qdrant::QdrantClient;
use kenjaku_infra::redis::{LocaleMemoryRedis, RedisClient};
use kenjaku_infra::telemetry::init_telemetry;
use kenjaku_infra::title_resolver::TitleResolver;
use kenjaku_infra::web_search::BraveSearchProvider;

use kenjaku_service::autocomplete::AutocompleteService;
use kenjaku_service::brain::{CompositeBrain, GeminiBrain};
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

    // Build the underlying GeminiBrain. One instance serves all three
    // Phase 2 sub-capabilities (Classifier, Translator, Generator).
    // Phase 3 can swap any of the three Arcs below to point at a
    // different provider without touching the pipeline or CompositeBrain.
    let gemini_brain = Arc::new(GeminiBrain::new(
        llm_provider.clone(),
        intent_classifier,
        use_google_search_tool,
        config.llm.model.clone(),
    ));

    let classifier: Arc<dyn Classifier> = gemini_brain.clone();
    let translator: Arc<dyn Translator> = gemini_brain.clone();
    let generator: Arc<dyn Generator> = gemini_brain.clone();

    let brain: Arc<dyn Brain> = Arc::new(CompositeBrain::new(classifier, translator, generator));

    // Phase 3b: the CollectionResolver is now wired into DocRagTool.
    // For every invocation the resolver maps the request's tenant to a
    // Qdrant collection name — `public` -> `{base}`, everything else ->
    // `{base}_{tenant}`. Until slice 3c, the handler injects
    // `TenantContext::public()` so the effective behavior is identical
    // to pre-3b (the resolver returns the bare base name).
    let collection_resolver: Arc<dyn CollectionResolver> = Arc::new(PrefixCollectionResolver::new(
        config.qdrant.collection_name.clone(),
    ));

    // Build the Tool list — DocRag (tier 1) then BraveWeb (tier 2).
    let doc_rag: Arc<dyn Tool> = Arc::new(DocRagTool::new(
        retriever,
        collection_resolver,
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

    // ------------------------------------------------------------------
    // Phase 3c.2 wiring — TenantsCache + JwtValidator + tenancy config.
    //
    // TenantsCache: snapshot of the `tenants` table loaded once at
    // startup. The `public` row seeded by migration
    // `20260415000001_add_tenant_id.up.sql` is always present, so the
    // cache is non-empty even on a fresh deploy.
    //
    // JwtValidator: built only when `tenancy.enabled = true`. The auth
    // middleware short-circuits to `TenantContext::public()` when
    // disabled and NEVER reads `state.jwt_validator` on that branch
    // (see `disabled_mode_never_invokes_validator` test).
    // ------------------------------------------------------------------
    let tenants_cache = Arc::new(TenantsCache::load_at_startup(&pg_pool).await?);
    info!(tenant_count = tenants_cache.len(), "TenantsCache loaded");

    let jwt_validator = load_jwt_validator(&config.tenancy)?;
    info!(
        tenancy_enabled = config.tenancy.enabled,
        validator_constructed = jwt_validator.is_some(),
        "Tenancy auth state"
    );

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
        tenants_cache,
        jwt_validator,
        tenancy_config: config.tenancy.clone(),
        rate_limit_config: config.rate_limit.clone(),
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
        // FIXME(3d): cross-tenant locale leak when `tenancy.enabled`
        // flips on. The `ResolvedLocale` extractor runs as a
        // FromRequestParts extractor per-handler — BEFORE the auth
        // middleware runs — so at this lookup site we only have a
        // device session id, not a `TenantContext`. As a result every
        // session-locale read currently lands under the `public`
        // tenant's namespace.
        //
        // Today (3c.2) `tenancy.enabled = false`, so writes also land
        // under `public` and there's no leak path — the locale memory
        // is correctly siloed in a single-tenant world.
        //
        // 3d MUST address this in the same change that flips the flag:
        //   1. extend `SessionLocaleLookup::lookup` to take
        //      `&TenantContext`, OR
        //   2. re-layer the router so `ResolvedLocale` runs after the
        //      auth middleware (so a tctx is available to extract
        //      from request extensions), OR
        //   3. accept the locale-as-public design + document.
        //
        // Tracked as Phase 3b architect flag #2; see the 3c.2
        // `architect-3c2.md` for the rationale on why this lands in
        // 3d alongside the enable flip rather than in 3c.2.
        let tctx = kenjaku_core::types::tenant::TenantContext::public();
        self.0.lookup(&tctx, session_id).await
    }
}

/// Build the JWT validator from configuration.
///
/// - `tenancy.enabled = false` → returns `Ok(None)`. The auth
///   middleware never reads the validator on the disabled branch.
/// - `tenancy.enabled = true` → reads the public-key PEM from
///   `tenancy.jwt.public_key_path` with hardening:
///     1. `is_file()` check rejects `/dev/zero`, FIFOs, etc.
///     2. 16 KiB size cap (an RSA-4096 PEM is ~1.7 KB; 16 KiB is
///        ~10x the largest realistic key).
///     3. Single `std::fs::read` call — bounded I/O.
///
///   then constructs the validator with the parsed PEM bytes.
///
/// Audit-trail INFO log on success: emits the public-key path and
/// the SHA-256 fingerprint (first 8 hex chars) — never the bytes.
///
/// This is the **only filesystem touch** in the entire JWT path.
/// Matches the 3c.1 architecture decision to keep
/// `kenjaku_infra::auth::jwt` filesystem-free; the bootstrap is the
/// one audit point for key-material loading.
fn load_jwt_validator(cfg: &TenancyConfig) -> KenjakuResult<Option<Arc<JwtValidator>>> {
    if !cfg.enabled {
        return Ok(None);
    }
    // `validate_secrets` already ran (line 50) and asserts that
    // `tenancy.enabled=true` implies a populated `jwt` block; the
    // expect here is a startup invariant, not a user-facing path.
    let jwt = cfg
        .jwt
        .as_ref()
        .expect("validate_secrets must guarantee jwt block when enabled=true");

    const MAX_PEM_BYTES: u64 = 16 * 1024;

    let path = Path::new(&jwt.public_key_path);
    let meta = std::fs::metadata(path).map_err(|e| {
        Error::Config(format!(
            "JWT public_key_path {} cannot be stat'd: {e}",
            jwt.public_key_path
        ))
    })?;
    if !meta.is_file() {
        return Err(Error::Config(format!(
            "JWT public_key_path {} is not a regular file",
            jwt.public_key_path
        )));
    }
    if meta.len() > MAX_PEM_BYTES {
        return Err(Error::Config(format!(
            "JWT public_key_path {} exceeds {MAX_PEM_BYTES} byte cap (got {} bytes)",
            jwt.public_key_path,
            meta.len()
        )));
    }

    let pem = std::fs::read(path).map_err(|e| {
        Error::Config(format!(
            "JWT public_key_path {} read failed: {e}",
            jwt.public_key_path
        ))
    })?;
    let validator = JwtValidator::new(jwt, &pem)?;
    let fingerprint = sha256_first_8(&pem);
    info!(
        path = %jwt.public_key_path,
        algorithm = jwt.algorithm.as_str(),
        fingerprint = %fingerprint,
        "JWT validator constructed"
    );
    Ok(Some(Arc::new(validator)))
}

/// SHA-256 of `bytes`, first 4 bytes hex-encoded (8 chars). Used only
/// for the JWT public-key audit log — operators can correlate
/// "this fingerprint is what's deployed" without ever printing the
/// key material.
fn sha256_first_8(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    hex::encode(&out[..4])
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use kenjaku_core::config::{JwtAlgorithm, JwtConfig};

    fn tenancy_disabled() -> TenancyConfig {
        TenancyConfig {
            enabled: false,
            header_name: "X-Tenant-Id".into(),
            default_tenant: "public".into(),
            collection_name_template: "{base}_{tenant}".into(),
            jwt: None,
        }
    }

    fn tenancy_with_jwt(public_key_path: &str) -> TenancyConfig {
        TenancyConfig {
            enabled: true,
            header_name: "X-Tenant-Id".into(),
            default_tenant: "public".into(),
            collection_name_template: "{base}_{tenant}".into(),
            jwt: Some(JwtConfig {
                issuer: "kenjaku-test".into(),
                audience: "kenjaku-api".into(),
                public_key_path: public_key_path.into(),
                algorithm: JwtAlgorithm::RS256,
                clock_skew_secs: 30,
            }),
        }
    }

    // Test RSA-2048 public key — same constant used in
    // kenjaku-infra/src/auth/jwt.rs unit tests + kenjaku-api auth
    // middleware tests.
    const TEST_RSA_PUBLIC_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAwCUFrwPKbw2egiLr2NdI
X4/B2HR8LGARprJJrPFQ6c5p+LsUyeMbhkxzFbx2cVxX+/G2hxZeu7SWNI7lzCsu
/ZV/Mxth61xSQBWG+fhwktHyZf+EBAWCjF5X65JruFWEiGh/kq4emyR+8hjqn6XP
WB2+uhCnb/op7xfXFgvedp6wmAvHOuSiJ3ZlP6pMKNcOopqk/j8TgyB99gKpiivj
whmGK/jpygg8ob2z/TcW4FuJvzcBUgt4+ZDUe1/ezgdz6lOcejlF2phhnXeNBI5i
6aMWlSxbOLlwOPSNqA2k97YFu0snm9lxCOPLjtqM9XT2QXAJpx9MxctgPDe1ANzr
qwIDAQAB
-----END PUBLIC KEY-----
";

    #[test]
    fn load_jwt_validator_disabled_returns_none() {
        let v = load_jwt_validator(&tenancy_disabled()).unwrap();
        assert!(v.is_none(), "tenancy disabled => no validator built");
    }

    #[test]
    fn load_jwt_validator_missing_file_fails() {
        let cfg = tenancy_with_jwt("/nonexistent/path/to/jwt_pub.pem");
        let err = load_jwt_validator(&cfg).expect_err("missing file must fail");
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn load_jwt_validator_oversized_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized.pem");
        // 32 KiB of garbage — larger than the 16 KiB cap.
        let big = vec![b'A'; 32 * 1024];
        std::fs::write(&path, &big).unwrap();
        let cfg = tenancy_with_jwt(path.to_string_lossy().as_ref());
        let err = load_jwt_validator(&cfg).expect_err("oversized must fail");
        let msg = err.to_string();
        assert!(matches!(err, Error::Config(_)));
        assert!(
            msg.contains("16384 byte cap"),
            "error must reference the cap: {msg}"
        );
    }

    #[test]
    fn load_jwt_validator_ok_path_returns_validator() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(TEST_RSA_PUBLIC_PEM.as_bytes()).unwrap();
        let cfg = tenancy_with_jwt(f.path().to_string_lossy().as_ref());
        let v = load_jwt_validator(&cfg).unwrap();
        assert!(v.is_some(), "valid PEM must yield a validator");
    }

    #[test]
    fn sha256_first_8_is_8_hex_chars_and_deterministic() {
        let a = sha256_first_8(b"hello");
        let b = sha256_first_8(b"hello");
        let c = sha256_first_8(b"world");
        assert_eq!(a.len(), 8);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
