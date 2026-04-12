use std::hash::{DefaultHasher, Hash, Hasher};

use serde::{Deserialize, Serialize};

use super::intent::Intent;
use super::locale::Locale;
use super::search::{LlmSource, RetrievedChunk};

/// Stable identifier for a tool. String-typed so config files and logs
/// stay readable; the registry enforces uniqueness at boot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolId(pub String);

/// What the Harness hands a tool on invocation. Owned (not borrowed)
/// so tools can spawn work onto other tasks without lifetime gymnastics.
/// Cloning is cheap -- a handful of small strings per request.
#[derive(Debug, Clone)]
pub struct ToolRequest {
    pub query_raw: String,
    pub query_normalized: String,
    pub locale: Locale,
    pub intent: Intent,
    pub collection_name: String,
    pub top_k: usize,
    pub request_id: String,
    pub session_id: String,
}

/// What a tool returns. Tagged enum so non-document tools don't have to
/// shoehorn their payload into chunk shape.
#[derive(Debug, Clone)]
pub enum ToolOutput {
    /// Document RAG and FAQ retrieval -- already chunk-shaped.
    Chunks {
        chunks: Vec<RetrievedChunk>,
        provider: String,
    },
    /// Live web search hits.
    WebHits {
        hits: Vec<LlmSource>,
        provider: String,
    },
    /// Structured payload (price quotes, FX, account lookups, etc.).
    Structured {
        facts: serde_json::Value,
        provider: String,
    },
    /// Tool ran but had nothing to contribute.
    Empty,
}

/// Per-tool error. Distinct from `kenjaku_core::Error` so the Harness
/// decides whether to degrade or propagate.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("tool disabled by config or rollout")]
    Disabled,
    #[error("tool timeout ({0}ms)")]
    Timeout(u64),
    #[error("upstream: {0}")]
    Upstream(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("cancelled")]
    Cancelled,
}

/// Default config for a tool. Rollout policy stays uniform while
/// tool-specific knobs live next to the impl.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Percentage rollout, 0.0-1.0. `None` means unconditional.
    #[serde(default)]
    pub rollout_pct: Option<f32>,
}

fn default_true() -> bool {
    true
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rollout_pct: None,
        }
    }
}

impl ToolConfig {
    /// Deterministic gate: hash `request_id` to decide whether a tool fires
    /// for this specific request. Same `request_id` always yields the same
    /// result, preventing flapping mid-stream.
    pub fn should_fire_for(&self, request_id: &str) -> bool {
        if !self.enabled {
            return false;
        }
        match self.rollout_pct {
            None => true,
            Some(pct) => {
                let mut hasher = DefaultHasher::new();
                request_id.hash(&mut hasher);
                let hash = hasher.finish();
                // Map hash to 0.0..1.0
                let normalized = (hash as f64) / (u64::MAX as f64);
                (normalized as f32) < pct
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_config_disabled_never_fires() {
        let config = ToolConfig {
            enabled: false,
            rollout_pct: None,
        };
        assert!(!config.should_fire_for("any-request-id"));
    }

    #[test]
    fn tool_config_enabled_no_rollout_always_fires() {
        let config = ToolConfig::default();
        assert!(config.should_fire_for("any-request-id"));
    }

    #[test]
    fn tool_config_rollout_deterministic() {
        let config = ToolConfig {
            enabled: true,
            rollout_pct: Some(0.5),
        };
        let request_id = "test-request-12345";
        let first = config.should_fire_for(request_id);
        // Must return the same result across multiple calls
        for _ in 0..100 {
            assert_eq!(config.should_fire_for(request_id), first);
        }
    }

    #[test]
    fn tool_config_rollout_zero_never_fires() {
        let config = ToolConfig {
            enabled: true,
            rollout_pct: Some(0.0),
        };
        assert!(!config.should_fire_for("any-request-id"));
    }

    #[test]
    fn tool_config_rollout_one_always_fires() {
        let config = ToolConfig {
            enabled: true,
            rollout_pct: Some(1.0),
        };
        // Every request should fire at 100% rollout
        for i in 0..50 {
            assert!(config.should_fire_for(&format!("request-{i}")));
        }
    }
}
