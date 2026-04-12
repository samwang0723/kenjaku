use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use kenjaku_core::traits::retriever::Retriever;
use kenjaku_core::traits::tool::Tool;
use kenjaku_core::types::tool::{
    ToolConfig, ToolError, ToolId, ToolOutput, ToolOutputMap, ToolRequest,
};

/// Wraps the existing `HybridRetriever` (or any `dyn Retriever`) as a
/// `Tool`. Phase 1: shadow implementation exercised only by tests.
pub struct DocRagTool {
    retriever: Arc<dyn Retriever>,
    collection_name: String,
    config: ToolConfig,
}

impl DocRagTool {
    pub fn new(retriever: Arc<dyn Retriever>, collection_name: String, config: ToolConfig) -> Self {
        Self {
            retriever,
            collection_name,
            config,
        }
    }
}

#[async_trait]
impl Tool for DocRagTool {
    fn id(&self) -> ToolId {
        ToolId("doc_rag".into())
    }

    fn config(&self) -> &ToolConfig {
        &self.config
    }

    fn should_fire(&self, req: &ToolRequest, _prior: &ToolOutputMap) -> bool {
        self.config.should_fire_for(&req.request_id)
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

        let chunks = self
            .retriever
            .retrieve(&req.query_normalized, &self.collection_name, req.top_k)
            .await
            .map_err(|e| ToolError::Upstream(e.to_string()))?;

        Ok(ToolOutput::Chunks {
            chunks,
            provider: "doc_rag".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenjaku_core::error::Result;
    use kenjaku_core::types::intent::Intent;
    use kenjaku_core::types::locale::Locale;
    use kenjaku_core::types::search::RetrievedChunk;

    struct MockRetriever;

    #[async_trait]
    impl Retriever for MockRetriever {
        async fn retrieve(
            &self,
            _query: &str,
            _collection: &str,
            _top_k: usize,
        ) -> Result<Vec<RetrievedChunk>> {
            Ok(vec![])
        }
    }

    fn make_request() -> ToolRequest {
        ToolRequest {
            query_raw: "test query".into(),
            query_normalized: "test query".into(),
            locale: Locale::En,
            intent: Intent::Factual,
            collection_name: "documents".into(),
            top_k: 10,
            request_id: "req-123".into(),
            session_id: "sess-456".into(),
        }
    }

    #[test]
    fn doc_rag_tool_should_fire_when_enabled() {
        let tool = DocRagTool::new(
            Arc::new(MockRetriever),
            "documents".into(),
            ToolConfig::default(),
        );
        let req = make_request();
        assert!(tool.should_fire(&req, &ToolOutputMap::new()));
    }

    #[test]
    fn doc_rag_tool_should_not_fire_when_disabled() {
        let tool = DocRagTool::new(
            Arc::new(MockRetriever),
            "documents".into(),
            ToolConfig {
                enabled: false,
                rollout_pct: None,
            },
        );
        let req = make_request();
        assert!(!tool.should_fire(&req, &ToolOutputMap::new()));
    }

    #[tokio::test]
    async fn doc_rag_tool_invoke_returns_chunks() {
        let tool = DocRagTool::new(
            Arc::new(MockRetriever),
            "documents".into(),
            ToolConfig::default(),
        );
        let req = make_request();
        let cancel = CancellationToken::new();
        let result = tool.invoke(&req, &ToolOutputMap::new(), &cancel).await;
        assert!(result.is_ok());
        match result.unwrap() {
            ToolOutput::Chunks { chunks, provider } => {
                assert!(chunks.is_empty());
                assert_eq!(provider, "doc_rag");
            }
            _ => panic!("expected ToolOutput::Chunks"),
        }
    }

    #[tokio::test]
    async fn doc_rag_tool_cancel_before_invoke() {
        let tool = DocRagTool::new(
            Arc::new(MockRetriever),
            "documents".into(),
            ToolConfig::default(),
        );
        let req = make_request();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = tool.invoke(&req, &ToolOutputMap::new(), &cancel).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::Cancelled => {}
            other => panic!("expected ToolError::Cancelled, got {other:?}"),
        }
    }
}
