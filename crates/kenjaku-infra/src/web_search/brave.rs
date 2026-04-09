//! Brave Search API client. Replaces Gemini's non-functional built-in
//! `google_search` grounding with a real web tier.
//!
//! Free tier: 2000 queries/month, 1 req/sec. Docs:
//! https://api.search.brave.com/app/documentation/web-search/get-started

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, instrument, warn};

use kenjaku_core::config::WebSearchConfig;
use kenjaku_core::error::{Error, Result};
use kenjaku_core::traits::web_search::{WebSearchProvider, WebSearchResult};

const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

pub struct BraveSearchProvider {
    client: Client,
    config: WebSearchConfig,
}

impl BraveSearchProvider {
    pub fn new(config: WebSearchConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|e| Error::Internal(format!("Failed to build Brave HTTP client: {e}")))?;
        Ok(Self { client, config })
    }
}

#[async_trait]
impl WebSearchProvider for BraveSearchProvider {
    #[instrument(skip(self), fields(limit = limit))]
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<WebSearchResult>> {
        if self.config.api_key.is_empty() {
            return Err(Error::Config(
                "Brave web_search.api_key is not configured".to_string(),
            ));
        }

        // Brave's `count` parameter caps at 20, and at least 1.
        let count = limit.clamp(1, 20);

        let response = self
            .client
            .get(BRAVE_ENDPOINT)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &self.config.api_key)
            .query(&[
                ("q", query),
                ("count", &count.to_string()),
                // Concise response — we only use title / url / description.
                ("text_decorations", "false"),
                ("safesearch", "moderate"),
            ])
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Brave request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Brave returned non-2xx");
            return Err(Error::Internal(format!("Brave returned {status}")));
        }

        let parsed: BraveResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("Failed to parse Brave response: {e}")))?;

        let results = parsed
            .web
            .map(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .take(limit)
            .map(|r| WebSearchResult {
                title: r.title,
                url: r.url,
                snippet: r.description,
            })
            .collect::<Vec<_>>();

        debug!(count = results.len(), "Brave search returned results");
        Ok(results)
    }
}

// ---------- Brave REST response types (minimal — only fields we use) ----------

#[derive(Debug, Deserialize)]
struct BraveResponse {
    #[serde(default)]
    web: Option<BraveWeb>,
}

#[derive(Debug, Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveWebResult>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
}
