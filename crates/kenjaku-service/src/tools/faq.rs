use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use kenjaku_core::traits::tool::Tool;
use kenjaku_core::types::search::{RetrievalMethod, RetrievedChunk};
use kenjaku_core::types::tool::{ToolConfig, ToolError, ToolId, ToolOutput, ToolOutputMap, ToolRequest};

/// A single FAQ entry loaded from configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaqEntry {
    pub question: String,
    pub answer: String,
    pub keywords: Vec<String>,
}

/// FAQ-specific configuration wrapping the shared `ToolConfig`.
#[derive(Debug, Clone)]
pub struct FaqToolConfig {
    pub base: ToolConfig,
    pub entries: Vec<FaqEntry>,
}

/// A simple FAQ retrieval tool that matches user queries against a static
/// list of frequently asked questions. Intentionally minimal — exists to
/// validate that the `Tool` trait plugin architecture works without editing
/// `harness/`, `brain/`, or `core/traits/`.
pub struct FaqTool {
    config: FaqToolConfig,
}

impl FaqTool {
    pub fn new(config: FaqToolConfig) -> Self {
        Self { config }
    }

    /// Case-insensitive check: does `query` contain `keyword`?
    fn query_matches_keyword(query: &str, keyword: &str) -> bool {
        query.to_lowercase().contains(&keyword.to_lowercase())
    }

    /// Returns true if any keyword from any entry appears in the query.
    fn any_keyword_matches(&self, query: &str) -> bool {
        self.config
            .entries
            .iter()
            .any(|entry| entry.keywords.iter().any(|kw| Self::query_matches_keyword(query, kw)))
    }
}

#[async_trait]
impl Tool for FaqTool {
    fn id(&self) -> ToolId {
        ToolId("faq".into())
    }

    fn config(&self) -> &ToolConfig {
        &self.config.base
    }

    fn should_fire(&self, req: &ToolRequest, _prior: &ToolOutputMap) -> bool {
        if !self.config.base.should_fire_for(&req.request_id) {
            return false;
        }
        self.any_keyword_matches(&req.query_normalized)
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

        let query_lower = req.query_normalized.to_lowercase();
        let matching_chunks: Vec<RetrievedChunk> = self
            .config
            .entries
            .iter()
            .filter(|entry| {
                entry
                    .keywords
                    .iter()
                    .any(|kw| query_lower.contains(&kw.to_lowercase()))
            })
            .enumerate()
            .map(|(i, entry)| RetrievedChunk {
                doc_id: format!("faq-{i}"),
                chunk_id: format!("faq-{i}-0"),
                title: entry.question.clone(),
                original_content: entry.answer.clone(),
                contextualized_content: entry.answer.clone(),
                source_url: None,
                score: 0.9,
                retrieval_method: RetrievalMethod::Hybrid, // closest semantic match
            })
            .collect();

        if matching_chunks.is_empty() {
            Ok(ToolOutput::Empty)
        } else {
            Ok(ToolOutput::Chunks {
                chunks: matching_chunks,
                provider: "faq".into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenjaku_core::types::intent::Intent;
    use kenjaku_core::types::locale::Locale;

    fn sample_entries() -> Vec<FaqEntry> {
        vec![
            FaqEntry {
                question: "How do I reset my password?".into(),
                answer: "Go to Settings > Security > Reset Password. You'll receive a verification email.".into(),
                keywords: vec!["password".into(), "reset".into(), "forgot".into(), "login".into()],
            },
            FaqEntry {
                question: "What payment methods are accepted?".into(),
                answer: "We accept Visa, Mastercard, bank transfer, and cryptocurrency payments.".into(),
                keywords: vec!["payment".into(), "pay".into(), "visa".into(), "mastercard".into(), "bank".into(), "crypto".into()],
            },
            FaqEntry {
                question: "How do I contact support?".into(),
                answer: "Email support@example.com or use the in-app chat feature.".into(),
                keywords: vec!["support".into(), "help".into(), "contact".into(), "email".into(), "chat".into()],
            },
        ]
    }

    fn make_tool(enabled: bool) -> FaqTool {
        FaqTool::new(FaqToolConfig {
            base: ToolConfig {
                enabled,
                rollout_pct: None,
            },
            entries: sample_entries(),
        })
    }

    fn make_request(query: &str) -> ToolRequest {
        ToolRequest {
            query_raw: query.into(),
            query_normalized: query.into(),
            locale: Locale::En,
            intent: Intent::Factual,
            collection_name: "documents".into(),
            top_k: 10,
            request_id: "req-faq-test".into(),
            session_id: "sess-faq-test".into(),
        }
    }

    #[tokio::test]
    async fn faq_tool_matches_keyword() {
        let tool = make_tool(true);
        let req = make_request("reset password");
        let cancel = CancellationToken::new();

        let result = tool.invoke(&req, &ToolOutputMap::new(), &cancel).await.unwrap();
        match result {
            ToolOutput::Chunks { chunks, provider } => {
                assert_eq!(provider, "faq");
                assert!(!chunks.is_empty());
                assert_eq!(chunks[0].title, "How do I reset my password?");
                assert!(chunks[0].original_content.contains("Settings > Security"));
                assert!((chunks[0].score - 0.9).abs() < f32::EPSILON);
            }
            other => panic!("expected ToolOutput::Chunks, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn faq_tool_no_match() {
        let tool = make_tool(true);
        let req = make_request("bitcoin price");
        let cancel = CancellationToken::new();

        let result = tool.invoke(&req, &ToolOutputMap::new(), &cancel).await.unwrap();
        assert!(
            matches!(result, ToolOutput::Empty),
            "expected ToolOutput::Empty for unmatched query"
        );
    }

    #[test]
    fn faq_tool_should_fire_when_keyword_present() {
        let tool = make_tool(true);
        let req = make_request("how do I reset my password");
        assert!(tool.should_fire(&req, &ToolOutputMap::new()));
    }

    #[test]
    fn faq_tool_should_not_fire_when_disabled() {
        let tool = make_tool(false);
        let req = make_request("reset password");
        assert!(!tool.should_fire(&req, &ToolOutputMap::new()));
    }

    #[tokio::test]
    async fn faq_tool_respects_cancellation() {
        let tool = make_tool(true);
        let req = make_request("reset password");
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
