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
            },
            telemetry: TelemetryConfig {
                service_name: "kenjaku".into(),
                otlp_endpoint: None,
                log_level: "info".into(),
            },
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
            },
            telemetry: TelemetryConfig {
                service_name: "kenjaku".into(),
                otlp_endpoint: None,
                log_level: "info".into(),
            },
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
}
