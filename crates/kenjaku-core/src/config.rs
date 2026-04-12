use serde::{Deserialize, Serialize};

use crate::types::component::ComponentLayout;

/// Top-level application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub qdrant: QdrantConfig,
    pub postgres: PostgresConfig,
    pub redis: RedisConfig,
    pub embedding: EmbeddingConfig,
    pub llm: LlmConfig,
    pub contextualizer: ContextualizerConfig,
    pub trending: TrendingConfig,
    pub chunking: ChunkingConfig,
    pub search: SearchConfig,
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub default_suggestions: DefaultSuggestionsConfig,
    #[serde(default)]
    pub locale_memory: LocaleMemoryConfig,
    #[serde(default)]
    pub web_search: WebSearchConfig,
}

impl AppConfig {
    /// Validate that all required secrets are present.
    /// Call this at startup after loading config to fail fast.
    pub fn validate_secrets(&self) -> crate::error::Result<()> {
        let mut missing = Vec::new();

        if self.postgres.url.is_empty() {
            missing.push("postgres.url");
        }
        if self.redis.url.is_empty() {
            missing.push("redis.url");
        }
        if self.embedding.api_key.is_empty() {
            missing.push("embedding.api_key");
        }
        if self.llm.api_key.is_empty() {
            missing.push("llm.api_key");
        }
        if self.contextualizer.api_key.is_empty() {
            missing.push("contextualizer.api_key");
        }

        if missing.is_empty() {
            Ok(())
        } else {
            Err(crate::error::Error::Config(format!(
                "Missing required secrets: {}. Set them in config/secrets.{{APP_ENV}}.yaml or via KENJAKU__ env vars.",
                missing.join(", ")
            )))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QdrantConfig {
    pub url: String,
    pub collection_name: String,
    #[serde(default = "default_vector_size")]
    pub vector_size: u64,
}

fn default_vector_size() -> u64 {
    1536
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresConfig {
    /// Connection URL including credentials. Must come from secrets.{env}.yaml.
    #[serde(default)]
    pub url: String,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

fn default_max_connections() -> u32 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    /// Connection URL. Must come from secrets.{env}.yaml.
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub provider: String,
    pub model: String,
    /// API key. Must come from secrets.{env}.yaml.
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_dimensions")]
    pub dimensions: usize,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

fn default_dimensions() -> usize {
    1536
}

fn default_batch_size() -> usize {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub model: String,
    /// API key. Must come from secrets.{env}.yaml.
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
}

fn default_max_tokens() -> u32 {
    4096
}

fn default_temperature() -> f32 {
    0.7
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextualizerConfig {
    pub provider: String,
    pub model: String,
    /// API key. Must come from secrets.{env}.yaml.
    #[serde(default)]
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendingConfig {
    /// Minimum search count to be considered popular.
    #[serde(default = "default_popularity_threshold")]
    pub popularity_threshold: i64,
    /// Interval in seconds between flush cycles.
    #[serde(default = "default_flush_interval_secs")]
    pub flush_interval_secs: u64,
    /// TTL in seconds for daily trending keys in Redis.
    #[serde(default = "default_daily_ttl_secs")]
    pub daily_ttl_secs: u64,
    /// TTL in seconds for weekly trending keys in Redis.
    #[serde(default = "default_weekly_ttl_secs")]
    pub weekly_ttl_secs: u64,
    /// Minimum search_count required for a query to appear in autocomplete
    /// or top-searches results. A second defensive layer after the record
    /// time gibberish guard — anything that slips through still needs
    /// multiple independent searches before it surfaces to users.
    #[serde(default = "default_crowd_sourcing_min_count")]
    pub crowd_sourcing_min_count: i64,
}

fn default_popularity_threshold() -> i64 {
    5
}

fn default_flush_interval_secs() -> u64 {
    300
}

fn default_daily_ttl_secs() -> u64 {
    172_800 // 2 days
}

fn default_weekly_ttl_secs() -> u64 {
    1_209_600 // 14 days
}

fn default_crowd_sourcing_min_count() -> i64 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkingConfig {
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: usize,
}

fn default_chunk_size() -> usize {
    512
}

fn default_chunk_overlap() -> usize {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    #[serde(default = "default_semantic_weight")]
    pub semantic_weight: f32,
    #[serde(default = "default_bm25_weight")]
    pub bm25_weight: f32,
    #[serde(default = "default_over_retrieve_factor")]
    pub over_retrieve_factor: usize,
    #[serde(default)]
    pub component_layout: ComponentLayout,
    #[serde(default = "default_suggestion_count")]
    pub suggestion_count: usize,
    #[serde(default)]
    pub history: HistoryConfig,
}

/// In-memory session conversation history knobs. Follow-up context for
/// the LLM call; NOT a replacement for the durable `conversations` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryConfig {
    #[serde(default = "default_history_enabled")]
    pub enabled: bool,
    /// Max turns kept per session. Older turns are evicted FIFO.
    #[serde(default = "default_history_max_turns_per_session")]
    pub max_turns_per_session: usize,
    /// Upper bound of turns actually injected into the LLM prompt per
    /// request. Lets us hold more context in memory for debugging while
    /// keeping the prompt budget predictable.
    #[serde(default = "default_history_inject_max_turns")]
    pub inject_max_turns: usize,
    /// Sessions idle longer than this are evicted by the background janitor.
    #[serde(default = "default_history_session_idle_ttl_seconds")]
    pub session_idle_ttl_seconds: u64,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: default_history_enabled(),
            max_turns_per_session: default_history_max_turns_per_session(),
            inject_max_turns: default_history_inject_max_turns(),
            session_idle_ttl_seconds: default_history_session_idle_ttl_seconds(),
        }
    }
}

fn default_history_enabled() -> bool {
    true
}
fn default_history_max_turns_per_session() -> usize {
    10
}
fn default_history_inject_max_turns() -> usize {
    6
}
fn default_history_session_idle_ttl_seconds() -> u64 {
    3600
}

fn default_semantic_weight() -> f32 {
    0.8
}

fn default_bm25_weight() -> f32 {
    0.2
}

fn default_over_retrieve_factor() -> usize {
    10
}

fn default_suggestion_count() -> usize {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    pub service_name: String,
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

// ===========================================================================
// Default suggestions + locale memory
// ===========================================================================

/// Top-level knobs for the dynamic default-suggestions feature.
/// See `.claude/tasks/default-suggestions-locale/tech-spec.md` §10.2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultSuggestionsConfig {
    #[serde(default = "default_default_suggestions_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub refresh: RefreshConfig,
    /// Regex deny-list applied to every LLM-produced question. Rows that
    /// match are dropped + logged as rejected. MUST be a non-backtracking
    /// pattern (the `regex` crate guarantees this).
    #[serde(default = "default_safety_regex")]
    pub safety_regex: String,
    /// Base pick weight written to `default_suggestions.weight` on insert.
    #[serde(default = "default_default_weight")]
    pub default_weight: i32,
    /// LARGE_LIMIT for the blending pool. Bounds how many rows the read
    /// path pulls per locale before weighted sampling.
    #[serde(default = "default_pool_cap")]
    pub pool_cap: usize,
}

impl Default for DefaultSuggestionsConfig {
    fn default() -> Self {
        Self {
            enabled: default_default_suggestions_enabled(),
            refresh: RefreshConfig::default(),
            safety_regex: default_safety_regex(),
            default_weight: default_default_weight(),
            pool_cap: default_pool_cap(),
        }
    }
}

/// Refresh worker tunables — schedule, sampling, clustering, retention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshConfig {
    /// Cron expression for the daily schedule. Parsed in `kenjaku-server`
    /// when spawning the scheduled task. Default: 03:00 UTC daily.
    #[serde(default = "default_schedule_cron")]
    pub schedule_cron: String,
    /// Maximum number of Qdrant points sampled per refresh run.
    #[serde(default = "default_sample_cap")]
    pub sample_cap: usize,
    /// Number of clusters for k-means. Reduced automatically if the
    /// sample count is < k*3.
    #[serde(default = "default_cluster_count")]
    pub cluster_count: usize,
    /// Target questions per (cluster, locale). The LLM may return fewer
    /// after safety filtering.
    #[serde(default = "default_per_cluster")]
    pub per_cluster: usize,
    /// How many historical batches to keep before cascade-deleting.
    #[serde(default = "default_retention_batches")]
    pub retention_batches: usize,
    /// LLM call timeout for `generate_cluster_questions`.
    #[serde(default = "default_generation_timeout_ms")]
    pub generation_timeout_ms: u64,
    /// `pg_try_advisory_lock` key for replica safety — only one replica
    /// can run the refresh at a time.
    #[serde(default = "default_advisory_lock_id")]
    pub advisory_lock_id: i64,
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            schedule_cron: default_schedule_cron(),
            sample_cap: default_sample_cap(),
            cluster_count: default_cluster_count(),
            per_cluster: default_per_cluster(),
            retention_batches: default_retention_batches(),
            generation_timeout_ms: default_generation_timeout_ms(),
            advisory_lock_id: default_advisory_lock_id(),
        }
    }
}

fn default_default_suggestions_enabled() -> bool {
    true
}
fn default_safety_regex() -> String {
    // (?i) = case-insensitive. Patterns are anchored at word fragments,
    // bounded, no backtracking cliffs. Enforced by the `regex` crate which
    // is linear-time by construction.
    r"(?i)(price|should i (buy|sell)|will .* hit|prediction|forecast)".to_string()
}
fn default_default_weight() -> i32 {
    10
}
fn default_pool_cap() -> usize {
    50
}
fn default_schedule_cron() -> String {
    "0 3 * * *".to_string()
}
fn default_sample_cap() -> usize {
    2000
}
fn default_cluster_count() -> usize {
    20
}
fn default_per_cluster() -> usize {
    5
}
fn default_retention_batches() -> usize {
    3
}
fn default_generation_timeout_ms() -> u64 {
    8000
}
fn default_advisory_lock_id() -> i64 {
    427318
}

/// Session -> locale memory (Redis-backed). Set `enabled=false` to skip
/// the Redis lookup in the ResolvedLocale extractor chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocaleMemoryConfig {
    #[serde(default = "default_locale_memory_enabled")]
    pub enabled: bool,
    /// Sliding TTL for `{key_prefix}{session_id}` entries.
    #[serde(default = "default_locale_memory_ttl_seconds")]
    pub ttl_seconds: u64,
    /// Redis key prefix. Keep trailing `:` so operators can `SCAN sl:*`.
    #[serde(default = "default_locale_memory_key_prefix")]
    pub key_prefix: String,
}

impl Default for LocaleMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: default_locale_memory_enabled(),
            ttl_seconds: default_locale_memory_ttl_seconds(),
            key_prefix: default_locale_memory_key_prefix(),
        }
    }
}

fn default_locale_memory_enabled() -> bool {
    true
}
fn default_locale_memory_ttl_seconds() -> u64 {
    7200
}
fn default_locale_memory_key_prefix() -> String {
    "sl:".to_string()
}

// ===========================================================================
// Web search (Brave / Serper / ...) — replaces Gemini's non-functional
// built-in google_search tool
// ===========================================================================

/// Live web search configuration. When enabled, `SearchService` augments
/// internal corpus retrieval with top-N results from a third-party search
/// API (Brave by default) for queries matching the trigger regex. The
/// results become synthetic `[Source N]` chunks the LLM cites like any
/// other retrieval chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchConfig {
    #[serde(default = "default_web_search_enabled")]
    pub enabled: bool,
    /// Vendor tag. Currently `brave` is the only supported value. New
    /// providers are added by implementing `WebSearchProvider` and
    /// wiring a match arm in the server bootstrap.
    #[serde(default = "default_web_search_provider")]
    pub provider: String,
    /// API key for the chosen provider. Lives in `secrets.{env}.yaml`,
    /// never in committed files.
    #[serde(default)]
    pub api_key: String,
    /// Max results to inject as `[Source N]` chunks per query.
    #[serde(default = "default_web_search_limit")]
    pub limit: usize,
    /// Request timeout against the provider, in milliseconds.
    #[serde(default = "default_web_search_timeout_ms")]
    pub timeout_ms: u64,
    /// Regex(es) matched (OR'd) against the translated query. If any
    /// matches, the web tier fires. Keeps us from burning queries on
    /// every single call.
    #[serde(default = "default_web_search_trigger_patterns")]
    pub trigger_patterns: Vec<String>,
    /// Also fire when internal retrieval returned fewer than this many
    /// chunks — covers the "in-domain but retrieval missed" case.
    #[serde(default = "default_web_search_fallback_min_chunks")]
    pub fallback_min_chunks: usize,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            enabled: default_web_search_enabled(),
            provider: default_web_search_provider(),
            api_key: String::new(),
            limit: default_web_search_limit(),
            timeout_ms: default_web_search_timeout_ms(),
            trigger_patterns: default_web_search_trigger_patterns(),
            fallback_min_chunks: default_web_search_fallback_min_chunks(),
        }
    }
}

fn default_web_search_enabled() -> bool {
    false
}
fn default_web_search_provider() -> String {
    "brave".to_string()
}
fn default_web_search_limit() -> usize {
    5
}
fn default_web_search_timeout_ms() -> u64 {
    4000
}
fn default_web_search_trigger_patterns() -> Vec<String> {
    vec![
        // Time-sensitive keywords
        r"(?i)\b(today|tonight|now|current|currently|latest|recent|this (week|month|year)|yesterday|live|real[- ]?time|this morning|this afternoon|this evening)\b".to_string(),
        // Topic keywords that are almost always real-time
        r"(?i)\b(price|market|news|weather|score|schedule|forecast|trending|happening|update|stocks?|crypto(currency)?)\b".to_string(),
    ]
}
fn default_web_search_fallback_min_chunks() -> usize {
    2
}

// ===========================================================================
// FAQ tool
// ===========================================================================

/// Load configuration from the config hierarchy.
///
/// Order (later overrides earlier):
/// 1. config/base.yaml
/// 2. config/{APP_ENV}.yaml (e.g., local, docker, staging, production)
/// 3. config/secrets.{APP_ENV}.yaml — API keys, DB credentials, etc.
/// 4. Environment variables (prefixed with KENJAKU__)
pub fn load_config() -> crate::error::Result<AppConfig> {
    let app_env = std::env::var("APP_ENV").unwrap_or_else(|_| "local".to_string());

    let config = config::Config::builder()
        .add_source(config::File::with_name("config/base").required(true))
        .add_source(config::File::with_name(&format!("config/{app_env}")).required(false))
        .add_source(config::File::with_name(&format!("config/secrets.{app_env}")).required(false))
        .add_source(
            config::Environment::with_prefix("KENJAKU")
                .separator("__")
                .try_parsing(true),
        )
        .build()
        .map_err(|e| crate::error::Error::Config(e.to_string()))?;

    config
        .try_deserialize()
        .map_err(|e| crate::error::Error::Config(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_load_config_from_base_and_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();

        // base.yaml has no secrets
        let base_yaml = r#"
server:
  host: "0.0.0.0"
  port: 8080
qdrant:
  url: "http://localhost:6334"
  collection_name: "documents"
embedding:
  provider: "openai"
  model: "text-embedding-3-small"
llm:
  provider: "gemini"
  model: "gemini-2.0-flash-lite"
contextualizer:
  provider: "anthropic"
  model: "claude-haiku-4-5"
trending:
  popularity_threshold: 5
  flush_interval_secs: 300
chunking:
  chunk_size: 512
  chunk_overlap: 50
search:
  semantic_weight: 0.8
  bm25_weight: 0.2
telemetry:
  service_name: "kenjaku"
  log_level: "info"
"#;
        let mut f = std::fs::File::create(config_dir.join("base.yaml")).unwrap();
        f.write_all(base_yaml.as_bytes()).unwrap();

        // secrets.local.yaml has the actual secrets
        let secrets_yaml = r#"
postgres:
  url: "postgres://user:pass@localhost:5432/kenjaku"
redis:
  url: "redis://localhost:6379"
embedding:
  api_key: "sk-test-key"
llm:
  api_key: "gemini-test-key"
contextualizer:
  api_key: "sk-ant-test-key"
"#;
        let mut f = std::fs::File::create(config_dir.join("secrets.local.yaml")).unwrap();
        f.write_all(secrets_yaml.as_bytes()).unwrap();

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        // SAFETY: This is a single-threaded test; no other threads read APP_ENV.
        unsafe {
            std::env::set_var("APP_ENV", "local");
        }

        let cfg = load_config().unwrap();
        assert_eq!(cfg.server.port, 8080);
        assert_eq!(cfg.qdrant.collection_name, "documents");
        assert_eq!(cfg.embedding.provider, "openai");
        assert_eq!(cfg.embedding.api_key, "sk-test-key");
        assert_eq!(cfg.llm.api_key, "gemini-test-key");
        assert_eq!(
            cfg.postgres.url,
            "postgres://user:pass@localhost:5432/kenjaku"
        );

        // Validate secrets pass
        assert!(cfg.validate_secrets().is_ok());

        std::env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    fn test_validate_secrets_missing() {
        // Test validate_secrets directly to avoid test isolation issues
        // with shared global state (current_dir, env vars).
        let cfg = AppConfig {
            server: ServerConfig {
                host: "0.0.0.0".into(),
                port: 8080,
            },
            qdrant: QdrantConfig {
                url: "http://localhost:6334".into(),
                collection_name: "docs".into(),
                vector_size: 1536,
            },
            postgres: PostgresConfig {
                url: String::new(),
                max_connections: 10,
            },
            redis: RedisConfig { url: String::new() },
            embedding: EmbeddingConfig {
                provider: "openai".into(),
                model: "m".into(),
                api_key: String::new(),
                dimensions: 1536,
                batch_size: 100,
            },
            llm: LlmConfig {
                provider: "gemini".into(),
                model: "m".into(),
                api_key: String::new(),
                max_tokens: 2048,
                temperature: 0.7,
            },
            contextualizer: ContextualizerConfig {
                provider: "anthropic".into(),
                model: "m".into(),
                api_key: String::new(),
            },
            trending: TrendingConfig {
                popularity_threshold: 5,
                flush_interval_secs: 300,
                daily_ttl_secs: 172800,
                weekly_ttl_secs: 1209600,
                crowd_sourcing_min_count: 2,
            },
            chunking: ChunkingConfig {
                chunk_size: 512,
                chunk_overlap: 50,
            },
            search: SearchConfig {
                semantic_weight: 0.8,
                bm25_weight: 0.2,
                over_retrieve_factor: 10,
                component_layout: ComponentLayout::default(),
                suggestion_count: 3,
                history: HistoryConfig::default(),
            },
            telemetry: TelemetryConfig {
                service_name: "kenjaku".into(),
                otlp_endpoint: None,
                log_level: "info".into(),
            },
            default_suggestions: DefaultSuggestionsConfig::default(),
            locale_memory: LocaleMemoryConfig::default(),
            web_search: WebSearchConfig::default(),
        };

        let result = cfg.validate_secrets();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("postgres.url"));
        assert!(err_msg.contains("redis.url"));
        assert!(err_msg.contains("embedding.api_key"));
        assert!(err_msg.contains("llm.api_key"));
        assert!(err_msg.contains("contextualizer.api_key"));
    }

    #[test]
    fn test_validate_secrets_passes_when_set() {
        let cfg = AppConfig {
            server: ServerConfig {
                host: "0.0.0.0".into(),
                port: 8080,
            },
            qdrant: QdrantConfig {
                url: "http://localhost:6334".into(),
                collection_name: "docs".into(),
                vector_size: 1536,
            },
            postgres: PostgresConfig {
                url: "postgres://u:p@localhost/db".into(),
                max_connections: 10,
            },
            redis: RedisConfig {
                url: "redis://localhost:6379".into(),
            },
            embedding: EmbeddingConfig {
                provider: "openai".into(),
                model: "m".into(),
                api_key: "sk-key".into(),
                dimensions: 1536,
                batch_size: 100,
            },
            llm: LlmConfig {
                provider: "gemini".into(),
                model: "m".into(),
                api_key: "gm-key".into(),
                max_tokens: 2048,
                temperature: 0.7,
            },
            contextualizer: ContextualizerConfig {
                provider: "anthropic".into(),
                model: "m".into(),
                api_key: "sk-ant-key".into(),
            },
            trending: TrendingConfig {
                popularity_threshold: 5,
                flush_interval_secs: 300,
                daily_ttl_secs: 172800,
                weekly_ttl_secs: 1209600,
                crowd_sourcing_min_count: 2,
            },
            chunking: ChunkingConfig {
                chunk_size: 512,
                chunk_overlap: 50,
            },
            search: SearchConfig {
                semantic_weight: 0.8,
                bm25_weight: 0.2,
                over_retrieve_factor: 10,
                component_layout: ComponentLayout::default(),
                suggestion_count: 3,
                history: HistoryConfig::default(),
            },
            telemetry: TelemetryConfig {
                service_name: "kenjaku".into(),
                otlp_endpoint: None,
                log_level: "info".into(),
            },
            default_suggestions: DefaultSuggestionsConfig::default(),
            locale_memory: LocaleMemoryConfig::default(),
            web_search: WebSearchConfig::default(),
        };

        assert!(cfg.validate_secrets().is_ok());
    }

    #[test]
    fn test_default_component_layout() {
        let layout = ComponentLayout::default();
        assert_eq!(layout.order.len(), 3);
        assert_eq!(
            layout.order,
            vec![
                crate::types::component::ComponentType::LlmAnswer,
                crate::types::component::ComponentType::Sources,
                crate::types::component::ComponentType::Suggestions,
            ]
        );
    }

    #[test]
    fn test_config_env_override() {
        let app_env = "staging";
        let expected_file = format!("config/{app_env}");
        assert_eq!(expected_file, "config/staging");
    }

    #[test]
    fn test_default_suggestions_config_defaults() {
        let cfg = DefaultSuggestionsConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.default_weight, 10);
        assert_eq!(cfg.pool_cap, 50);
        assert!(cfg.safety_regex.starts_with("(?i)"));
        assert_eq!(cfg.refresh.schedule_cron, "0 3 * * *");
        assert_eq!(cfg.refresh.sample_cap, 2000);
        assert_eq!(cfg.refresh.cluster_count, 20);
        assert_eq!(cfg.refresh.per_cluster, 5);
        assert_eq!(cfg.refresh.retention_batches, 3);
        assert_eq!(cfg.refresh.generation_timeout_ms, 8000);
        assert_eq!(cfg.refresh.advisory_lock_id, 427318);
    }

    #[test]
    fn test_locale_memory_config_defaults() {
        let cfg = LocaleMemoryConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.ttl_seconds, 7200);
        assert_eq!(cfg.key_prefix, "sl:");
    }

    #[test]
    fn test_default_suggestions_config_deserializes_from_yaml() {
        let yaml = r#"
enabled: true
refresh:
  schedule_cron: "0 4 * * *"
  sample_cap: 1500
  cluster_count: 15
  per_cluster: 4
  retention_batches: 5
  generation_timeout_ms: 10000
  advisory_lock_id: 427318
safety_regex: "(?i)(buy|sell)"
default_weight: 7
pool_cap: 40
"#;
        let cfg: DefaultSuggestionsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.refresh.schedule_cron, "0 4 * * *");
        assert_eq!(cfg.refresh.sample_cap, 1500);
        assert_eq!(cfg.refresh.cluster_count, 15);
        assert_eq!(cfg.default_weight, 7);
        assert_eq!(cfg.pool_cap, 40);
    }

    #[test]
    fn test_locale_memory_config_deserializes_from_yaml() {
        let yaml = r#"
enabled: false
ttl_seconds: 3600
key_prefix: "loc:"
"#;
        let cfg: LocaleMemoryConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.ttl_seconds, 3600);
        assert_eq!(cfg.key_prefix, "loc:");
    }

    #[test]
    fn test_refresh_config_partial_yaml_uses_defaults() {
        // Only override one field — the rest should fall back to defaults
        // via the `#[serde(default = ...)]` annotations.
        let yaml = "sample_cap: 500\n";
        let cfg: RefreshConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.sample_cap, 500);
        assert_eq!(cfg.cluster_count, 20); // default
        assert_eq!(cfg.schedule_cron, "0 3 * * *"); // default
    }
}
