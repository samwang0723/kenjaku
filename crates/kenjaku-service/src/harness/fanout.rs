use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

use kenjaku_core::error::Error;
use kenjaku_core::traits::tool::Tool;
use kenjaku_core::types::tool::{ToolError, ToolId, ToolOutput, ToolOutputMap, ToolRequest};

/// Dependency-aware DAG executor for tool fan-out.
///
/// Tools declare dependencies via `Tool::depends_on()`. The tunnel resolves
/// execution tiers via topological sort (Kahn's algorithm) at construction
/// time. Within a tier, tools run in parallel via `join_all`. Between tiers,
/// execution is sequential so that tier N tools can read tier 0..N-1 outputs
/// via the `ToolOutputMap`.
///
/// Error policy:
/// - `ToolError::Upstream | Timeout | Cancelled | Disabled` -> `ToolOutput::Empty`
/// - `ToolError::BadRequest` -> propagated as `Error::Validation` (fails the request)
pub struct ToolTunnel {
    /// Tools grouped by execution tier. Tier 0 has no deps, Tier 1 depends
    /// on Tier 0 outputs, etc. Within a tier, tools run in parallel.
    tiers: Vec<Vec<Arc<dyn Tool>>>,
}

impl ToolTunnel {
    /// Build from a flat tool list. Topologically sorts by `depends_on()`.
    /// Panics on dependency cycles. Logs a warning for unregistered deps
    /// (tool will see `None` in the output map -- can decide to skip).
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        // Build lookup: tool_id -> index. Detect duplicates eagerly so
        // misconfigured registries fail at boot, not at runtime.
        let mut id_to_idx: HashMap<String, usize> = HashMap::with_capacity(tools.len());
        for (i, t) in tools.iter().enumerate() {
            let id = t.id().0.clone();
            if let Some(prev) = id_to_idx.insert(id.clone(), i) {
                panic!(
                    "ToolTunnel: duplicate tool ID '{id}' at indices {prev} and {i}; \
                     each tool must have a unique ID"
                );
            }
        }

        let n = tools.len();

        // Build adjacency and in-degree.
        // edge: dependency -> dependent (dep must complete before dependent)
        let mut in_degree = vec![0usize; n];
        let mut dependents: Vec<Vec<usize>> = vec![vec![]; n];

        for (idx, tool) in tools.iter().enumerate() {
            for dep_id in tool.depends_on() {
                if let Some(&dep_idx) = id_to_idx.get(&dep_id.0) {
                    dependents[dep_idx].push(idx);
                    in_degree[idx] += 1;
                } else {
                    warn!(
                        tool = %tool.id().0,
                        dependency = %dep_id.0,
                        "tool depends on unregistered tool; dependency will be absent from output map"
                    );
                }
            }
        }

        // Kahn's algorithm: peel layers of in-degree 0 nodes.
        let mut tiers: Vec<Vec<Arc<dyn Tool>>> = Vec::new();
        let mut remaining = n;
        let mut current_in_degree = in_degree;

        loop {
            let tier_indices: Vec<usize> = (0..n).filter(|&i| current_in_degree[i] == 0).collect();

            if tier_indices.is_empty() {
                break;
            }

            // Mark these as processed by setting in-degree to usize::MAX.
            for &idx in &tier_indices {
                current_in_degree[idx] = usize::MAX;
                remaining -= 1;
                // Decrement dependents' in-degree.
                for &dep_idx in &dependents[idx] {
                    if current_in_degree[dep_idx] != usize::MAX {
                        current_in_degree[dep_idx] -= 1;
                    }
                }
            }

            let tier: Vec<Arc<dyn Tool>> = tier_indices
                .into_iter()
                .map(|i| Arc::clone(&tools[i]))
                .collect();
            tiers.push(tier);
        }

        assert!(
            remaining == 0,
            "ToolTunnel: dependency cycle detected among {} tools; \
             remaining tools could not be placed in any tier",
            remaining,
        );

        Self { tiers }
    }

    /// Number of tiers resolved.
    #[cfg(test)]
    pub fn tier_count(&self) -> usize {
        self.tiers.len()
    }

    /// Tool IDs in a given tier (for testing).
    #[cfg(test)]
    pub fn tier_tool_ids(&self, tier: usize) -> Vec<String> {
        self.tiers
            .get(tier)
            .map(|t| t.iter().map(|tool| tool.id().0.clone()).collect())
            .unwrap_or_default()
    }

    /// Execute all tiers sequentially, tools within each tier in parallel.
    /// Returns the accumulated output map, or `Error::Validation` if any
    /// tool returns `ToolError::BadRequest`.
    pub async fn execute(
        &self,
        req: &ToolRequest,
        cancel: &CancellationToken,
        tool_budget_ms: u64,
    ) -> Result<ToolOutputMap, Error> {
        let timeout_dur = Duration::from_millis(tool_budget_ms);
        let mut accumulated = ToolOutputMap::new();

        for tier in &self.tiers {
            let firing: Vec<_> = tier
                .iter()
                .filter(|t| t.should_fire(req, &accumulated))
                .collect();

            if firing.is_empty() {
                continue;
            }

            let results: Vec<(ToolId, Result<ToolOutput, Error>)> =
                futures::future::join_all(firing.iter().map(|t| {
                    let t = Arc::clone(t);
                    let req = req.clone();
                    let prior = accumulated.clone();
                    let cancel = cancel.clone();
                    async move {
                        let id = t.id();
                        let output = match tokio::time::timeout(
                            timeout_dur,
                            t.invoke(&req, &prior, &cancel),
                        )
                        .await
                        {
                            Ok(Ok(out)) => Ok(out),
                            Ok(Err(ToolError::BadRequest(msg))) => {
                                error!(tool = %id.0, error = %msg, "tool bad request");
                                Err(Error::Validation(format!("tool '{}': {msg}", id.0)))
                            }
                            Ok(Err(ToolError::Disabled)) => Ok(ToolOutput::Empty),
                            Ok(Err(e)) => {
                                warn!(tool = %id.0, error = %e, "tool degraded to empty");
                                Ok(ToolOutput::Empty)
                            }
                            Err(_) => {
                                warn!(
                                    tool = %id.0,
                                    budget_ms = tool_budget_ms,
                                    "tool timed out"
                                );
                                Ok(ToolOutput::Empty)
                            }
                        };
                        (id, output)
                    }
                }))
                .await;

            for (id, result) in results {
                match result {
                    Ok(output) => accumulated.insert(id, output),
                    Err(e) => return Err(e),
                }
            }
        }

        Ok(accumulated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use kenjaku_core::types::intent::Intent;
    use kenjaku_core::types::locale::Locale;
    use kenjaku_core::types::search::{LlmSource, RetrievedChunk};
    use kenjaku_core::types::tool::ToolConfig;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ---- helpers -----------------------------------------------------------

    fn make_request() -> ToolRequest {
        use kenjaku_core::types::tenant::test_helpers::public_test_context;
        ToolRequest::new(
            "test".into(),
            "test".into(),
            Locale::En,
            Intent::Factual,
            "docs".into(),
            10,
            "req-1".into(),
            "sess-1".into(),
            &public_test_context(),
        )
    }

    static TOOL_CONFIG: ToolConfig = ToolConfig {
        enabled: true,
        rollout_pct: None,
    };

    // ---- mock tools --------------------------------------------------------

    /// A tool with configurable id, deps, output, and fire behavior.
    struct MockTool {
        tool_id: String,
        deps: Vec<ToolId>,
        output: ToolOutput,
        fire: bool,
    }

    impl MockTool {
        fn new(id: &str) -> Self {
            Self {
                tool_id: id.into(),
                deps: vec![],
                output: ToolOutput::Empty,
                fire: true,
            }
        }

        fn with_deps(mut self, deps: Vec<&str>) -> Self {
            self.deps = deps.into_iter().map(|d| ToolId(d.into())).collect();
            self
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
    impl Tool for MockTool {
        fn id(&self) -> ToolId {
            ToolId(self.tool_id.clone())
        }
        fn config(&self) -> &ToolConfig {
            &TOOL_CONFIG
        }
        fn depends_on(&self) -> Vec<ToolId> {
            self.deps.clone()
        }
        fn should_fire(&self, _req: &ToolRequest, _prior: &ToolOutputMap) -> bool {
            self.fire
        }
        async fn invoke(
            &self,
            _req: &ToolRequest,
            _prior: &ToolOutputMap,
            _cancel: &CancellationToken,
        ) -> Result<ToolOutput, ToolError> {
            Ok(self.output.clone())
        }
    }

    /// A tool that always returns an Upstream error.
    struct FailTool {
        tool_id: String,
    }

    #[async_trait]
    impl Tool for FailTool {
        fn id(&self) -> ToolId {
            ToolId(self.tool_id.clone())
        }
        fn config(&self) -> &ToolConfig {
            &TOOL_CONFIG
        }
        fn should_fire(&self, _req: &ToolRequest, _prior: &ToolOutputMap) -> bool {
            true
        }
        async fn invoke(
            &self,
            _req: &ToolRequest,
            _prior: &ToolOutputMap,
            _cancel: &CancellationToken,
        ) -> Result<ToolOutput, ToolError> {
            Err(ToolError::Upstream("boom".into()))
        }
    }

    /// A tool that records its invocation order via an atomic counter.
    struct OrderTool {
        tool_id: String,
        deps: Vec<ToolId>,
        counter: Arc<AtomicUsize>,
        order: Arc<std::sync::Mutex<Vec<(String, usize)>>>,
    }

    #[async_trait]
    impl Tool for OrderTool {
        fn id(&self) -> ToolId {
            ToolId(self.tool_id.clone())
        }
        fn config(&self) -> &ToolConfig {
            &TOOL_CONFIG
        }
        fn depends_on(&self) -> Vec<ToolId> {
            self.deps.clone()
        }
        fn should_fire(&self, _req: &ToolRequest, _prior: &ToolOutputMap) -> bool {
            true
        }
        async fn invoke(
            &self,
            _req: &ToolRequest,
            _prior: &ToolOutputMap,
            _cancel: &CancellationToken,
        ) -> Result<ToolOutput, ToolError> {
            let seq = self.counter.fetch_add(1, Ordering::SeqCst);
            self.order.lock().unwrap().push((self.tool_id.clone(), seq));
            Ok(ToolOutput::Empty)
        }
    }

    /// A tool that only fires when doc_rag returned < threshold chunks.
    struct ConditionalTool {
        threshold: usize,
    }

    #[async_trait]
    impl Tool for ConditionalTool {
        fn id(&self) -> ToolId {
            ToolId("conditional".into())
        }
        fn config(&self) -> &ToolConfig {
            &TOOL_CONFIG
        }
        fn depends_on(&self) -> Vec<ToolId> {
            vec![ToolId("doc_rag".into())]
        }
        fn should_fire(&self, _req: &ToolRequest, prior: &ToolOutputMap) -> bool {
            prior.chunk_count("doc_rag") < self.threshold
        }
        async fn invoke(
            &self,
            _req: &ToolRequest,
            _prior: &ToolOutputMap,
            _cancel: &CancellationToken,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::WebHits {
                hits: vec![LlmSource {
                    title: "web result".into(),
                    url: "https://example.com".into(),
                    snippet: Some("snippet".into()),
                }],
                provider: "test".into(),
            })
        }
    }

    // ---- ToolTunnel construction tests ------------------------------------

    #[test]
    fn tunnel_resolves_tiers_correctly() {
        // A (no deps), C (no deps) -> Tier 0
        // B (depends on A) -> Tier 1
        let a = Arc::new(MockTool::new("a")) as Arc<dyn Tool>;
        let b = Arc::new(MockTool::new("b").with_deps(vec!["a"])) as Arc<dyn Tool>;
        let c = Arc::new(MockTool::new("c")) as Arc<dyn Tool>;

        let tunnel = ToolTunnel::new(vec![a, b, c]);
        assert_eq!(tunnel.tier_count(), 2);

        let tier0 = tunnel.tier_tool_ids(0);
        assert!(tier0.contains(&"a".to_string()));
        assert!(tier0.contains(&"c".to_string()));
        assert_eq!(tier0.len(), 2);

        let tier1 = tunnel.tier_tool_ids(1);
        assert_eq!(tier1, vec!["b".to_string()]);
    }

    #[test]
    #[should_panic(expected = "dependency cycle detected")]
    fn tunnel_detects_cycle() {
        let a = Arc::new(MockTool::new("a").with_deps(vec!["b"])) as Arc<dyn Tool>;
        let b = Arc::new(MockTool::new("b").with_deps(vec!["a"])) as Arc<dyn Tool>;
        ToolTunnel::new(vec![a, b]);
    }

    #[test]
    fn tunnel_warns_missing_dep() {
        // B depends on "nonexistent" -- should not panic, B lands in Tier 0
        // because unregistered deps are warned but not counted as in-degree.
        let b = Arc::new(MockTool::new("b").with_deps(vec!["nonexistent"])) as Arc<dyn Tool>;
        let tunnel = ToolTunnel::new(vec![b]);
        assert_eq!(tunnel.tier_count(), 1);
        assert_eq!(tunnel.tier_tool_ids(0), vec!["b".to_string()]);
    }

    #[tokio::test]
    async fn tunnel_executes_tiers_in_order() {
        let counter = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        let a = Arc::new(OrderTool {
            tool_id: "a".into(),
            deps: vec![],
            counter: counter.clone(),
            order: order.clone(),
        }) as Arc<dyn Tool>;

        let b = Arc::new(OrderTool {
            tool_id: "b".into(),
            deps: vec![ToolId("a".into())],
            counter: counter.clone(),
            order: order.clone(),
        }) as Arc<dyn Tool>;

        let tunnel = ToolTunnel::new(vec![a, b]);
        let req = make_request();
        let cancel = CancellationToken::new();
        tunnel.execute(&req, &cancel, 5000).await.unwrap();

        let log = order.lock().unwrap();
        // "a" must have a lower sequence number than "b"
        let a_seq = log.iter().find(|(id, _)| id == "a").unwrap().1;
        let b_seq = log.iter().find(|(id, _)| id == "b").unwrap().1;
        assert!(
            a_seq < b_seq,
            "Tier 0 tool 'a' must execute before Tier 1 tool 'b'"
        );
    }

    #[tokio::test]
    async fn tunnel_parallel_within_tier() {
        // Two tier-0 tools: both should execute and appear in the output map.
        let a = Arc::new(MockTool::new("a").with_output(ToolOutput::Empty)) as Arc<dyn Tool>;
        let b = Arc::new(MockTool::new("b").with_output(ToolOutput::Empty)) as Arc<dyn Tool>;

        let tunnel = ToolTunnel::new(vec![a, b]);
        let req = make_request();
        let cancel = CancellationToken::new();
        let outputs = tunnel.execute(&req, &cancel, 5000).await.unwrap();

        assert_eq!(outputs.len(), 2);
        assert!(outputs.get("a").is_some());
        assert!(outputs.get("b").is_some());
    }

    #[tokio::test]
    async fn tunnel_degrades_on_error() {
        let fail = Arc::new(FailTool {
            tool_id: "fail".into(),
        }) as Arc<dyn Tool>;

        let tunnel = ToolTunnel::new(vec![fail]);
        let req = make_request();
        let cancel = CancellationToken::new();
        let outputs = tunnel.execute(&req, &cancel, 5000).await.unwrap();

        assert_eq!(outputs.len(), 1);
        assert!(matches!(outputs.get("fail"), Some(ToolOutput::Empty)));
    }

    #[tokio::test]
    async fn tunnel_respects_should_fire() {
        let a = Arc::new(MockTool::new("a").with_fire(false)) as Arc<dyn Tool>;
        let tunnel = ToolTunnel::new(vec![a]);
        let req = make_request();
        let cancel = CancellationToken::new();
        let outputs = tunnel.execute(&req, &cancel, 5000).await.unwrap();
        assert!(outputs.is_empty());
    }

    #[tokio::test]
    async fn tunnel_tier2_fires_when_few_chunks() {
        // doc_rag returns 0 chunks -> conditional (threshold=2) should fire
        let doc_rag = Arc::new(MockTool::new("doc_rag").with_output(ToolOutput::Chunks {
            chunks: vec![],
            provider: "rag".into(),
        })) as Arc<dyn Tool>;

        let conditional = Arc::new(ConditionalTool { threshold: 2 }) as Arc<dyn Tool>;

        let tunnel = ToolTunnel::new(vec![doc_rag, conditional]);
        let req = make_request();
        let cancel = CancellationToken::new();
        let outputs = tunnel.execute(&req, &cancel, 5000).await.unwrap();

        assert_eq!(outputs.len(), 2);
        assert!(outputs.get("doc_rag").is_some());
        assert!(matches!(
            outputs.get("conditional"),
            Some(ToolOutput::WebHits { hits, .. }) if hits.len() == 1
        ));
    }

    #[tokio::test]
    async fn tunnel_tier2_skipped_when_enough_chunks() {
        let chunk = RetrievedChunk {
            doc_id: "d1".into(),
            chunk_id: "c1".into(),
            title: "Title".into(),
            original_content: "content".into(),
            contextualized_content: "content".into(),
            source_url: Some("https://example.com".into()),
            score: 0.9,
            retrieval_method: kenjaku_core::types::search::RetrievalMethod::Vector,
        };

        let doc_rag = Arc::new(MockTool::new("doc_rag").with_output(ToolOutput::Chunks {
            chunks: vec![chunk.clone(), chunk.clone(), chunk],
            provider: "rag".into(),
        })) as Arc<dyn Tool>;

        // Fires only when prior < 2, but doc_rag has 3 chunks
        let conditional = Arc::new(ConditionalTool { threshold: 2 }) as Arc<dyn Tool>;

        let tunnel = ToolTunnel::new(vec![doc_rag, conditional]);
        let req = make_request();
        let cancel = CancellationToken::new();
        let outputs = tunnel.execute(&req, &cancel, 5000).await.unwrap();

        assert_eq!(outputs.len(), 1); // only doc_rag
        assert!(outputs.get("doc_rag").is_some());
        assert!(outputs.get("conditional").is_none());
    }

    #[test]
    #[should_panic(expected = "duplicate tool ID")]
    fn tunnel_detects_duplicate_tool_ids() {
        let a = Arc::new(MockTool::new("dup")) as Arc<dyn Tool>;
        let b = Arc::new(MockTool::new("dup")) as Arc<dyn Tool>;
        ToolTunnel::new(vec![a, b]);
    }

    /// A tool that always returns a BadRequest error.
    struct BadRequestTool {
        tool_id: String,
    }

    #[async_trait]
    impl Tool for BadRequestTool {
        fn id(&self) -> ToolId {
            ToolId(self.tool_id.clone())
        }
        fn config(&self) -> &ToolConfig {
            &TOOL_CONFIG
        }
        fn should_fire(&self, _req: &ToolRequest, _prior: &ToolOutputMap) -> bool {
            true
        }
        async fn invoke(
            &self,
            _req: &ToolRequest,
            _prior: &ToolOutputMap,
            _cancel: &CancellationToken,
        ) -> Result<ToolOutput, ToolError> {
            Err(ToolError::BadRequest("invalid query".into()))
        }
    }

    #[tokio::test]
    async fn tunnel_propagates_bad_request() {
        let tool = Arc::new(BadRequestTool {
            tool_id: "bad".into(),
        }) as Arc<dyn Tool>;

        let tunnel = ToolTunnel::new(vec![tool]);
        let req = make_request();
        let cancel = CancellationToken::new();
        let result = tunnel.execute(&req, &cancel, 5000).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match &err {
            Error::Validation(msg) => {
                assert!(msg.contains("bad"), "should mention tool id");
                assert!(
                    msg.contains("invalid query"),
                    "should contain the error message"
                );
            }
            other => panic!("expected Error::Validation, got: {other:?}"),
        }
    }
}
