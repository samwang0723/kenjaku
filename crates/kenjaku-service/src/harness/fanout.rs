use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::warn;

use kenjaku_core::traits::tool::Tool;
use kenjaku_core::types::tool::{ToolError, ToolOutput, ToolRequest};

/// Fan out tool invocations with per-tool timeout and graceful degradation.
///
/// Two-tier execution:
/// - **Tier 1** (index 0): `DocRagTool` always runs first. Its output
///   determines `prior_chunk_count` for tier-2 tools.
/// - **Tier 2** (indices 1..): Evaluated with `should_fire(req, prior_chunk_count)`
///   after tier 1 completes. Tools that pass fire via `join_all` (NOT
///   `try_join_all`) so one failure degrades to `ToolOutput::Empty` instead
///   of aborting the entire fan-out.
///
/// Error policy (per design doc SS4.3):
/// - `ToolError::Upstream | Timeout | Cancelled` -> `ToolOutput::Empty`
/// - `ToolError::Disabled` -> silent skip (not even logged)
/// - `ToolError::BadRequest` -> propagated as `kenjaku_core::Error::Validation`
pub async fn fanout_tools(
    tools: &[Arc<dyn Tool>],
    req: &ToolRequest,
    cancel: &CancellationToken,
    tool_budget_ms: u64,
) -> kenjaku_core::error::Result<Vec<ToolOutput>> {
    let timeout_dur = Duration::from_millis(tool_budget_ms);
    let mut outputs: Vec<ToolOutput> = Vec::with_capacity(tools.len());

    // --- Tier 1: first tool (DocRagTool) runs unconditionally ---------------
    if let Some(tier1) = tools.first()
        && tier1.should_fire(req, 0)
    {
        let out = invoke_with_timeout(tier1.as_ref(), req, cancel, timeout_dur).await?;
        outputs.push(out);
    }

    // Compute prior_chunk_count from tier-1 outputs for tier-2 gating.
    let prior_chunk_count = chunk_count_from_outputs(&outputs);

    // --- Tier 2: remaining tools, gated by should_fire(prior_chunk_count) ---
    if tools.len() > 1 {
        let tier2_futs: Vec<_> = tools[1..]
            .iter()
            .filter(|t| t.should_fire(req, prior_chunk_count))
            .map(|t| {
                let t = Arc::clone(t);
                let req = req.clone();
                let cancel = cancel.clone();
                async move { invoke_with_timeout(t.as_ref(), &req, &cancel, timeout_dur).await }
            })
            .collect();

        let tier2_results = futures::future::join_all(tier2_futs).await;
        for result in tier2_results {
            match result {
                Ok(out) => outputs.push(out),
                Err(e) => {
                    // BadRequest already propagated from invoke_with_timeout;
                    // this branch handles it if it somehow reaches here.
                    return Err(e);
                }
            }
        }
    }

    Ok(outputs)
}

/// Invoke a single tool with timeout + degradation.
///
/// Returns `Ok(ToolOutput)` on success or graceful degradation.
/// Returns `Err` only for `BadRequest` (which should fail the request).
async fn invoke_with_timeout(
    tool: &dyn Tool,
    req: &ToolRequest,
    cancel: &CancellationToken,
    timeout: Duration,
) -> kenjaku_core::error::Result<ToolOutput> {
    let tool_id = tool.id();
    match tokio::time::timeout(timeout, tool.invoke(req, cancel)).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(ToolError::BadRequest(msg))) => {
            Err(kenjaku_core::error::Error::Validation(msg))
        }
        Ok(Err(ToolError::Disabled)) => {
            // Silent skip per error policy.
            Ok(ToolOutput::Empty)
        }
        Ok(Err(e)) => {
            warn!(tool = %tool_id.0, error = %e, "tool failed; degrading to empty");
            Ok(ToolOutput::Empty)
        }
        Err(_elapsed) => {
            warn!(tool = %tool_id.0, timeout_ms = timeout.as_millis(), "tool timed out; degrading to empty");
            Ok(ToolOutput::Empty)
        }
    }
}

/// Count chunks across all outputs so far.
fn chunk_count_from_outputs(outputs: &[ToolOutput]) -> usize {
    outputs
        .iter()
        .map(|o| match o {
            ToolOutput::Chunks { chunks, .. } => chunks.len(),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use kenjaku_core::types::intent::Intent;
    use kenjaku_core::types::locale::Locale;
    use kenjaku_core::types::search::{LlmSource, RetrievedChunk};
    use kenjaku_core::types::tool::{ToolConfig, ToolId};

    // ---- helpers -----------------------------------------------------------

    fn make_request() -> ToolRequest {
        ToolRequest {
            query_raw: "test".into(),
            query_normalized: "test".into(),
            locale: Locale::En,
            intent: Intent::Factual,
            collection_name: "docs".into(),
            top_k: 10,
            request_id: "req-1".into(),
            session_id: "sess-1".into(),
        }
    }

    // ---- mock tools --------------------------------------------------------

    /// A tool that always succeeds with the given output.
    struct SuccessTool {
        output: ToolOutput,
        fire: bool,
    }

    #[async_trait]
    impl Tool for SuccessTool {
        fn id(&self) -> ToolId {
            ToolId("success".into())
        }
        fn config(&self) -> &ToolConfig {
            &ToolConfig {
                enabled: true,
                rollout_pct: None,
            }
        }
        fn should_fire(&self, _req: &ToolRequest, _prior: usize) -> bool {
            self.fire
        }
        async fn invoke(
            &self,
            _req: &ToolRequest,
            _cancel: &CancellationToken,
        ) -> Result<ToolOutput, ToolError> {
            Ok(self.output.clone())
        }
    }

    /// A tool that always returns an Upstream error.
    struct FailTool;

    #[async_trait]
    impl Tool for FailTool {
        fn id(&self) -> ToolId {
            ToolId("fail".into())
        }
        fn config(&self) -> &ToolConfig {
            &ToolConfig {
                enabled: true,
                rollout_pct: None,
            }
        }
        fn should_fire(&self, _req: &ToolRequest, _prior: usize) -> bool {
            true
        }
        async fn invoke(
            &self,
            _req: &ToolRequest,
            _cancel: &CancellationToken,
        ) -> Result<ToolOutput, ToolError> {
            Err(ToolError::Upstream("boom".into()))
        }
    }

    /// A tool that only fires when prior_chunk_count < threshold.
    struct ConditionalTool {
        threshold: usize,
    }

    #[async_trait]
    impl Tool for ConditionalTool {
        fn id(&self) -> ToolId {
            ToolId("conditional".into())
        }
        fn config(&self) -> &ToolConfig {
            &ToolConfig {
                enabled: true,
                rollout_pct: None,
            }
        }
        fn should_fire(&self, _req: &ToolRequest, prior: usize) -> bool {
            prior < self.threshold
        }
        async fn invoke(
            &self,
            _req: &ToolRequest,
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

    // ---- tests -------------------------------------------------------------

    #[tokio::test]
    async fn fanout_tools_degrades_on_error() {
        // Tier 1 tool fails with Upstream error -> should degrade to Empty
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FailTool)];
        let req = make_request();
        let cancel = CancellationToken::new();
        let outputs = fanout_tools(&tools, &req, &cancel, 5000).await.unwrap();
        assert_eq!(outputs.len(), 1);
        assert!(matches!(outputs[0], ToolOutput::Empty));
    }

    #[tokio::test]
    async fn fanout_tools_respects_should_fire() {
        // Tier 1 tool with fire=false should produce no output
        let tier1 = Arc::new(SuccessTool {
            output: ToolOutput::Chunks {
                chunks: vec![],
                provider: "rag".into(),
            },
            fire: false,
        }) as Arc<dyn Tool>;

        // Tier 2 conditional tool with high threshold should still not fire
        // because there are 0 prior chunks but the tier-1 didn't fire
        let tier2 = Arc::new(ConditionalTool { threshold: 0 }) as Arc<dyn Tool>;

        let tools: Vec<Arc<dyn Tool>> = vec![tier1, tier2];
        let req = make_request();
        let cancel = CancellationToken::new();
        let outputs = fanout_tools(&tools, &req, &cancel, 5000).await.unwrap();
        // Neither fired
        assert!(outputs.is_empty());
    }

    #[tokio::test]
    async fn fanout_tools_tier2_fires_when_few_chunks() {
        // Tier 1 returns 0 chunks
        let tier1 = Arc::new(SuccessTool {
            output: ToolOutput::Chunks {
                chunks: vec![],
                provider: "rag".into(),
            },
            fire: true,
        }) as Arc<dyn Tool>;

        // Tier 2 fires when prior < 2
        let tier2 = Arc::new(ConditionalTool { threshold: 2 }) as Arc<dyn Tool>;

        let tools = vec![tier1, tier2];
        let req = make_request();
        let cancel = CancellationToken::new();
        let outputs = fanout_tools(&tools, &req, &cancel, 5000).await.unwrap();
        // tier1 (empty chunks) + tier2 (web hits)
        assert_eq!(outputs.len(), 2);
        assert!(matches!(&outputs[0], ToolOutput::Chunks { chunks, .. } if chunks.is_empty()));
        assert!(matches!(&outputs[1], ToolOutput::WebHits { hits, .. } if hits.len() == 1));
    }

    #[tokio::test]
    async fn fanout_tools_tier2_skipped_when_enough_chunks() {
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
        let tier1 = Arc::new(SuccessTool {
            output: ToolOutput::Chunks {
                chunks: vec![chunk.clone(), chunk.clone(), chunk],
                provider: "rag".into(),
            },
            fire: true,
        }) as Arc<dyn Tool>;

        // Tier 2 fires only when prior < 2 -- tier1 has 3 chunks so skip
        let tier2 = Arc::new(ConditionalTool { threshold: 2 }) as Arc<dyn Tool>;

        let tools = vec![tier1, tier2];
        let req = make_request();
        let cancel = CancellationToken::new();
        let outputs = fanout_tools(&tools, &req, &cancel, 5000).await.unwrap();
        assert_eq!(outputs.len(), 1); // only tier1
    }
}
