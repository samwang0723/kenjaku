use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use serde::{Deserialize, Serialize};

use super::intent::Intent;
use super::locale::Locale;
use super::search::{LlmSource, RetrievedChunk};
use super::tenant::TenantContext;

/// Stable identifier for a tool. String-typed so config files and logs
/// stay readable; the registry enforces uniqueness at boot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolId(pub String);

/// What the Harness hands a tool on invocation. Owned (not borrowed)
/// so tools can spawn work onto other tasks without lifetime gymnastics.
/// Cloning is cheap -- a handful of small strings per request.
///
/// Construction goes through [`ToolRequest::new`]. The `tenant` field is
/// intentionally private (per the Phase 3a architect gate flag #2) so a
/// tool cannot rewrite a request's tenant in-place and accidentally (or
/// maliciously) cross tenant boundaries. Read-only access via
/// [`ToolRequest::tenant`].
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
    /// Request-scoped tenancy context. Private by design — tools read via
    /// [`ToolRequest::tenant`] and MUST NOT mutate it. In Phase 3b every
    /// request resolves to [`TenantContext::public`]; slice 3c populates
    /// this from the JWT extractor.
    tenant: TenantContext,
}

impl ToolRequest {
    /// Construct a `ToolRequest`. Clones `tctx` into an owned field so the
    /// request can move across async boundaries without lifetime
    /// gymnastics. Use this constructor — struct-literal construction
    /// won't compile because `tenant` is private.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        query_raw: String,
        query_normalized: String,
        locale: Locale,
        intent: Intent,
        collection_name: String,
        top_k: usize,
        request_id: String,
        session_id: String,
        tctx: &TenantContext,
    ) -> Self {
        Self {
            query_raw,
            query_normalized,
            locale,
            intent,
            collection_name,
            top_k,
            request_id,
            session_id,
            tenant: tctx.clone(),
        }
    }

    /// Borrow the request-scoped tenancy context.
    ///
    /// Tools use this to drive per-tenant behavior (e.g. the default
    /// `DocRagTool` hands `self.tenant().tenant_id` to a
    /// `CollectionResolver` so each tenant reads from its own Qdrant
    /// collection). In 3b this always returns `TenantContext::public()`.
    pub fn tenant(&self) -> &TenantContext {
        &self.tenant
    }
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
                // Map hash to [0.0, 1.0) — the +1.0 ensures u64::MAX
                // maps to a value strictly less than 1.0 so that
                // rollout_pct == 1.0 always fires.
                let normalized = (hash as f64) / (u64::MAX as f64 + 1.0);
                (normalized as f32) < pct
            }
        }
    }
}

/// Accumulated outputs from prior tool tiers, keyed by `ToolId`.
/// Tools in tier N can read outputs from tiers 0..N-1.
///
/// Maintains insertion order via `ordered_keys` so that iteration
/// via `iter_ordered()` is deterministic (important for stable
/// source numbering in merged chunks).
#[derive(Debug, Clone, Default)]
pub struct ToolOutputMap {
    map: HashMap<ToolId, ToolOutput>,
    ordered_keys: Vec<ToolId>,
}

impl ToolOutputMap {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            ordered_keys: Vec::new(),
        }
    }

    pub fn insert(&mut self, id: ToolId, output: ToolOutput) {
        if !self.map.contains_key(&id) {
            self.ordered_keys.push(id.clone());
        }
        self.map.insert(id, output);
    }

    pub fn get(&self, id: &str) -> Option<&ToolOutput> {
        self.map.get(&ToolId(id.to_string()))
    }

    /// Count chunks returned by a specific tool. Returns 0 if tool
    /// didn't fire or returned a non-Chunks variant.
    pub fn chunk_count(&self, tool_id: &str) -> usize {
        match self.get(tool_id) {
            Some(ToolOutput::Chunks { chunks, .. }) => chunks.len(),
            _ => 0,
        }
    }

    /// True if any tool returned `WebHits`.
    pub fn has_web_hits(&self) -> bool {
        self.map
            .values()
            .any(|o| matches!(o, ToolOutput::WebHits { .. }))
    }

    /// Iterate all outputs in insertion order. Use this instead of
    /// `iter()` when stable ordering matters (e.g. source numbering).
    pub fn iter_ordered(&self) -> impl Iterator<Item = (&ToolId, &ToolOutput)> {
        self.ordered_keys
            .iter()
            .filter_map(|k| self.map.get(k).map(|v| (k, v)))
    }

    /// Iterate all outputs (HashMap order — non-deterministic).
    /// Prefer `iter_ordered()` when stable ordering matters.
    pub fn iter(&self) -> impl Iterator<Item = (&ToolId, &ToolOutput)> {
        self.map.iter()
    }

    /// Consume into inner map.
    pub fn into_inner(self) -> HashMap<ToolId, ToolOutput> {
        self.map
    }

    /// Number of tool outputs stored.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tenant::TenantContext;

    // ---- ToolRequest tenant threading (Phase 3b) --------------------------

    fn sample_request(tctx: &TenantContext) -> ToolRequest {
        ToolRequest::new(
            "raw query".to_string(),
            "raw query".to_string(),
            Locale::En,
            Intent::Factual,
            "documents".to_string(),
            10,
            "req-1".to_string(),
            "sess-1".to_string(),
            tctx,
        )
    }

    #[test]
    fn tool_request_new_stores_tenant_and_accessor_returns_it() {
        let tctx = TenantContext::public();
        let req = sample_request(&tctx);
        assert_eq!(req.tenant().tenant_id.as_str(), "public");
        assert!(req.tenant().principal_id.is_none());
    }

    #[test]
    fn tool_request_new_clones_tctx_so_caller_retains_ownership() {
        // Caller's tctx must still be usable after ToolRequest::new consumed
        // a borrow.
        let tctx = TenantContext::public();
        let _req = sample_request(&tctx);
        // Would not compile if `new` took ownership.
        assert_eq!(tctx.tenant_id.as_str(), "public");
    }

    #[test]
    fn tool_request_clone_preserves_tenant() {
        let tctx = TenantContext::public();
        let req = sample_request(&tctx);
        let dup = req.clone();
        assert_eq!(dup.tenant().tenant_id.as_str(), "public");
    }

    // ---- Existing tests ---------------------------------------------------

    #[test]
    fn tool_output_map_chunk_count() {
        let mut map = ToolOutputMap::new();
        assert_eq!(map.chunk_count("doc_rag"), 0);

        map.insert(
            ToolId("doc_rag".into()),
            ToolOutput::Chunks {
                chunks: vec![],
                provider: "rag".into(),
            },
        );
        assert_eq!(map.chunk_count("doc_rag"), 0);
        assert_eq!(map.chunk_count("nonexistent"), 0);
    }

    #[test]
    fn tool_output_map_has_web_hits() {
        let mut map = ToolOutputMap::new();
        assert!(!map.has_web_hits());

        map.insert(
            ToolId("brave_web".into()),
            ToolOutput::WebHits {
                hits: vec![LlmSource {
                    title: "t".into(),
                    url: "u".into(),
                    snippet: None,
                }],
                provider: "brave".into(),
            },
        );
        assert!(map.has_web_hits());
    }

    #[test]
    fn tool_output_map_len_and_iter() {
        let mut map = ToolOutputMap::new();
        assert!(map.is_empty());
        map.insert(ToolId("a".into()), ToolOutput::Empty);
        map.insert(ToolId("b".into()), ToolOutput::Empty);
        assert_eq!(map.len(), 2);
        assert_eq!(map.iter().count(), 2);
    }

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
