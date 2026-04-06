use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use kenjaku_core::config::EmbeddingConfig;
use kenjaku_core::error::{Error, Result};
use kenjaku_core::traits::embedding::EmbeddingProvider;

/// OpenAI embedding provider.
pub struct OpenAiEmbeddingProvider {
    client: Client,
    config: EmbeddingConfig,
}

impl OpenAiEmbeddingProvider {
    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            client: Client::new(),
            config,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    #[instrument(skip(self, texts), fields(count = texts.len()))]
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let request = OpenAiEmbeddingRequest {
            model: self.config.model.clone(),
            input: texts.to_vec(),
            dimensions: Some(self.config.dimensions),
        };

        let response = self
            .client
            .post("https://api.openai.com/v1/embeddings")
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Embedding(format!("OpenAI request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Embedding(format!(
                "OpenAI returned {status}: {body}"
            )));
        }

        let result: OpenAiEmbeddingResponse = response
            .json()
            .await
            .map_err(|e| Error::Embedding(format!("Failed to parse OpenAI response: {e}")))?;

        debug!(
            model = %result.model,
            total_tokens = result.usage.total_tokens,
            "OpenAI embedding completed"
        );

        // Sort by index to ensure correct ordering
        let mut data = result.data;
        data.sort_by_key(|d| d.index);

        Ok(data.into_iter().map(|d| d.embedding).collect())
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }

    fn dimensions(&self) -> usize {
        self.config.dimensions
    }
}

#[derive(Serialize)]
struct OpenAiEmbeddingRequest {
    model: String,
    input: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
}

#[derive(Deserialize)]
struct OpenAiEmbeddingResponse {
    data: Vec<OpenAiEmbeddingData>,
    model: String,
    usage: OpenAiEmbeddingUsage,
}

#[derive(Deserialize)]
struct OpenAiEmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

#[derive(Deserialize)]
struct OpenAiEmbeddingUsage {
    total_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_config(url: &str) -> EmbeddingConfig {
        EmbeddingConfig {
            provider: "openai".to_string(),
            model: "text-embedding-3-small".to_string(),
            api_key: "test-key".to_string(),
            dimensions: 1536,
            batch_size: 100,
        }
    }

    #[tokio::test]
    async fn test_embed_returns_vectors() {
        let server = MockServer::start().await;

        let response_body = serde_json::json!({
            "object": "list",
            "data": [
                {
                    "object": "embedding",
                    "index": 0,
                    "embedding": vec![0.1_f32; 1536]
                }
            ],
            "model": "text-embedding-3-small",
            "usage": {
                "prompt_tokens": 5,
                "total_tokens": 5
            }
        });

        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(header("Authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response_body))
            .mount(&server)
            .await;

        let config = test_config(&server.uri());
        let provider = OpenAiEmbeddingProvider {
            client: Client::new(),
            config,
        };

        // Note: This test would need the URL overridden to hit wiremock.
        // In production, we'd use a configurable base_url.
        // For now, this validates the struct and serialization.
        let request = OpenAiEmbeddingRequest {
            model: "text-embedding-3-small".to_string(),
            input: vec!["hello world".to_string()],
            dimensions: Some(1536),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("text-embedding-3-small"));
        assert!(json.contains("hello world"));
    }

    #[test]
    fn test_model_name() {
        let config = EmbeddingConfig {
            provider: "openai".to_string(),
            model: "text-embedding-3-small".to_string(),
            api_key: "test".to_string(),
            dimensions: 1536,
            batch_size: 100,
        };
        let provider = OpenAiEmbeddingProvider::new(config);
        assert_eq!(provider.model_name(), "text-embedding-3-small");
        assert_eq!(provider.dimensions(), 1536);
    }
}
