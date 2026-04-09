//! Live web search trait. Used by `SearchService` to augment the internal
//! corpus retrieval with fresh web results for time-sensitive or
//! in-domain-but-missing queries.
//!
//! Design note: the built-in `google_search` grounding tool on Gemini
//! 3.1 preview is currently non-functional (the model silently ignores
//! the tool declaration and refuses with "I don't have real-time data").
//! This trait provides the replacement path — we execute the search
//! ourselves via a third-party API (Brave, Serper, …), inject the top
//! results as synthetic `[Source N]` chunks alongside internal corpus
//! retrieval, and let Gemini synthesize from the full combined context.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// One hit from a live web search provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    /// Brief description / snippet / abstract from the provider.
    pub snippet: String,
}

/// Live web search provider — one impl per vendor (Brave, Serper, etc).
#[async_trait]
pub trait WebSearchProvider: Send + Sync {
    /// Execute a web search and return up to `limit` results.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<WebSearchResult>>;
}
