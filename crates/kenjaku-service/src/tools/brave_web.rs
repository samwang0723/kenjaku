use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use kenjaku_core::traits::tool::Tool;
use kenjaku_core::traits::web_search::WebSearchProvider;
use kenjaku_core::types::search::LlmSource;
use kenjaku_core::types::tool::{
    ToolConfig, ToolError, ToolId, ToolOutput, ToolOutputMap, ToolRequest,
};

/// Wraps an optional `WebSearchProvider` (Brave, Serper, etc.) as a
/// `Tool`. Mirrors the existing `should_web_search` + `fetch_web_chunks`
/// logic from `SearchService`.
pub struct BraveWebTool {
    provider: Option<Arc<dyn WebSearchProvider>>,
    config: ToolConfig,
    /// Source patterns as strings -- kept for Debug.
    trigger_pattern_strings: Vec<String>,
    /// Compiled trigger regexes.
    trigger_patterns: Vec<Regex>,
    fallback_min_chunks: usize,
    /// Max results to request from the web search provider.
    limit: usize,
}

impl std::fmt::Debug for BraveWebTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BraveWebTool")
            .field("provider_present", &self.provider.is_some())
            .field("config", &self.config)
            .field("trigger_patterns", &self.trigger_pattern_strings)
            .field("fallback_min_chunks", &self.fallback_min_chunks)
            .field("limit", &self.limit)
            .finish()
    }
}

impl BraveWebTool {
    /// Create a new `BraveWebTool`. Bad patterns are dropped with a
    /// warning so a single misconfigured entry doesn't disable the whole
    /// web tier.
    pub fn new(
        provider: Option<Arc<dyn WebSearchProvider>>,
        config: ToolConfig,
        trigger_pattern_strings: Vec<String>,
        fallback_min_chunks: usize,
        limit: usize,
    ) -> Self {
        let trigger_patterns = trigger_pattern_strings
            .iter()
            .filter_map(|p| match Regex::new(p) {
                Ok(r) => Some(r),
                Err(e) => {
                    warn!(pattern = %p, error = %e, "dropping invalid web_search trigger pattern");
                    None
                }
            })
            .collect();

        Self {
            provider,
            config,
            trigger_pattern_strings,
            trigger_patterns,
            fallback_min_chunks,
            limit,
        }
    }
}

#[async_trait]
impl Tool for BraveWebTool {
    fn id(&self) -> ToolId {
        ToolId("brave_web".into())
    }

    fn config(&self) -> &ToolConfig {
        &self.config
    }

    fn depends_on(&self) -> Vec<ToolId> {
        vec![ToolId("doc_rag".into())]
    }

    /// Fires when:
    /// 1. Config allows (enabled + rollout check),
    /// 2. Provider is present,
    /// 3. Query matches any trigger pattern, OR internal retrieval
    ///    returned fewer than `fallback_min_chunks`.
    fn should_fire(&self, req: &ToolRequest, prior: &ToolOutputMap) -> bool {
        if !self.config.should_fire_for(&req.request_id) {
            return false;
        }
        if self.provider.is_none() {
            return false;
        }
        if self
            .trigger_patterns
            .iter()
            .any(|r| r.is_match(&req.query_normalized))
        {
            return true;
        }
        prior.chunk_count("doc_rag") < self.fallback_min_chunks
    }

    async fn invoke(
        &self,
        req: &ToolRequest,
        _prior: &ToolOutputMap,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }

        let provider = self.provider.as_ref().ok_or(ToolError::Disabled)?;

        let results = provider
            .search(&req.query_normalized, self.limit)
            .await
            .map_err(|e| ToolError::Upstream(e.to_string()))?;

        let hits: Vec<LlmSource> = results
            .into_iter()
            .map(|r| LlmSource {
                title: r.title,
                url: r.url,
                snippet: Some(r.snippet),
            })
            .collect();

        Ok(ToolOutput::WebHits {
            hits,
            provider: "brave".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenjaku_core::error::Result;
    use kenjaku_core::traits::web_search::WebSearchResult;
    use kenjaku_core::types::intent::Intent;
    use kenjaku_core::types::locale::Locale;

    struct MockWebSearchProvider {
        results: Vec<WebSearchResult>,
    }

    #[async_trait]
    impl WebSearchProvider for MockWebSearchProvider {
        async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<WebSearchResult>> {
            Ok(self.results.clone())
        }
    }

    fn make_request(query: &str) -> ToolRequest {
        use kenjaku_core::types::tenant::test_helpers::public_test_context;
        ToolRequest::new(
            query.into(),
            query.into(),
            Locale::En,
            Intent::Factual,
            "documents".into(),
            5,
            "req-123".into(),
            "sess-456".into(),
            &public_test_context(),
        )
    }

    fn default_trigger_patterns() -> Vec<String> {
        vec![
            r"(?i)\b(today|tonight|now|current|currently|latest|recent|this (week|month|year)|yesterday|live|real[- ]?time)\b".into(),
            r"(?i)\b(price|market|news|weather|score|forecast|trending|happening|update|stocks?|crypto(currency)?)\b".into(),
        ]
    }

    /// Build a `ToolOutputMap` with doc_rag returning `n` chunks.
    fn prior_with_chunks(n: usize) -> ToolOutputMap {
        use kenjaku_core::types::search::{RetrievalMethod, RetrievedChunk};
        let mut map = ToolOutputMap::new();
        let chunks: Vec<RetrievedChunk> = (0..n)
            .map(|i| RetrievedChunk {
                doc_id: format!("d{i}"),
                chunk_id: format!("c{i}"),
                title: "T".into(),
                original_content: "C".into(),
                contextualized_content: "C".into(),
                source_url: None,
                score: 0.9,
                retrieval_method: RetrievalMethod::Vector,
            })
            .collect();
        map.insert(
            ToolId("doc_rag".into()),
            ToolOutput::Chunks {
                chunks,
                provider: "rag".into(),
            },
        );
        map
    }

    #[test]
    fn brave_web_tool_should_fire_pattern_match() {
        let tool = BraveWebTool::new(
            Some(Arc::new(MockWebSearchProvider { results: vec![] })),
            ToolConfig::default(),
            default_trigger_patterns(),
            2,
            5,
        );
        // "market today" matches both trigger patterns
        let req = make_request("market today");
        assert!(tool.should_fire(&req, &ToolOutputMap::new()));
    }

    #[test]
    fn brave_web_tool_should_not_fire_no_pattern_enough_chunks() {
        let tool = BraveWebTool::new(
            Some(Arc::new(MockWebSearchProvider { results: vec![] })),
            ToolConfig::default(),
            default_trigger_patterns(),
            2,
            5,
        );
        // "reset password" does not match trigger patterns
        // and prior has 10 chunks >= fallback_min_chunks=2
        let req = make_request("reset password");
        assert!(!tool.should_fire(&req, &prior_with_chunks(10)));
    }

    #[test]
    fn brave_web_tool_should_fire_fallback_few_chunks() {
        let tool = BraveWebTool::new(
            Some(Arc::new(MockWebSearchProvider { results: vec![] })),
            ToolConfig::default(),
            default_trigger_patterns(),
            2,
            5,
        );
        // No pattern match but prior has 0 chunks < fallback_min_chunks=2
        let req = make_request("reset password");
        assert!(tool.should_fire(&req, &ToolOutputMap::new()));
    }

    #[test]
    fn brave_web_tool_should_not_fire_no_provider() {
        let tool = BraveWebTool::new(
            None,
            ToolConfig::default(),
            default_trigger_patterns(),
            2,
            5,
        );
        let req = make_request("market today");
        assert!(!tool.should_fire(&req, &ToolOutputMap::new()));
    }

    #[test]
    fn brave_web_tool_should_not_fire_disabled() {
        let tool = BraveWebTool::new(
            Some(Arc::new(MockWebSearchProvider { results: vec![] })),
            ToolConfig {
                enabled: false,
                rollout_pct: None,
            },
            default_trigger_patterns(),
            2,
            5,
        );
        let req = make_request("market today");
        assert!(!tool.should_fire(&req, &ToolOutputMap::new()));
    }

    #[tokio::test]
    async fn brave_web_tool_cancel() {
        let tool = BraveWebTool::new(
            Some(Arc::new(MockWebSearchProvider { results: vec![] })),
            ToolConfig::default(),
            default_trigger_patterns(),
            2,
            5,
        );
        let req = make_request("market today");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = tool.invoke(&req, &ToolOutputMap::new(), &cancel).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::Cancelled => {}
            other => panic!("expected ToolError::Cancelled, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn brave_web_tool_invoke_returns_web_hits() {
        let provider = MockWebSearchProvider {
            results: vec![WebSearchResult {
                title: "Test Result".into(),
                url: "https://example.com".into(),
                snippet: "A test snippet".into(),
            }],
        };
        let tool = BraveWebTool::new(
            Some(Arc::new(provider)),
            ToolConfig::default(),
            default_trigger_patterns(),
            2,
            5,
        );
        let req = make_request("market today");
        let cancel = CancellationToken::new();
        let result = tool
            .invoke(&req, &ToolOutputMap::new(), &cancel)
            .await
            .unwrap();
        match result {
            ToolOutput::WebHits { hits, provider } => {
                assert_eq!(hits.len(), 1);
                assert_eq!(hits[0].title, "Test Result");
                assert_eq!(hits[0].url, "https://example.com");
                assert_eq!(hits[0].snippet.as_deref(), Some("A test snippet"));
                assert_eq!(provider, "brave");
            }
            _ => panic!("expected ToolOutput::WebHits"),
        }
    }
}
