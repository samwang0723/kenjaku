use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use kenjaku_core::config::ContextualizerConfig;
use kenjaku_core::error::{Error, Result};
use kenjaku_core::traits::llm::Contextualizer;

const DOCUMENT_CONTEXT_PROMPT: &str = r#"<document>
{doc_content}
</document>"#;

const CHUNK_CONTEXT_PROMPT: &str = r#"Here is the chunk we want to situate within the whole document
<chunk>
{chunk_content}
</chunk>

Please give a short succinct context to situate this chunk within the overall document for the purposes of improving search retrieval of the chunk.
Answer only with the succinct context and nothing else."#;

/// Claude-based contextualizer for generating chunk context.
pub struct ClaudeContextualizer {
    client: Client,
    config: ContextualizerConfig,
    base_url: String,
}

impl ClaudeContextualizer {
    pub fn new(config: ContextualizerConfig) -> Self {
        Self {
            client: Client::new(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            config,
        }
    }

    /// Create with a custom base URL (for testing).
    pub fn with_base_url(config: ContextualizerConfig, base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
            config,
        }
    }
}

#[async_trait]
impl Contextualizer for ClaudeContextualizer {
    #[instrument(skip(self, document_content, chunk_content))]
    async fn contextualize(&self, document_content: &str, chunk_content: &str) -> Result<String> {
        let doc_prompt = DOCUMENT_CONTEXT_PROMPT.replace("{doc_content}", document_content);
        let chunk_prompt = CHUNK_CONTEXT_PROMPT.replace("{chunk_content}", chunk_content);

        let request = ClaudeRequest {
            model: self.config.model.clone(),
            max_tokens: 1024,
            messages: vec![ClaudeMessage {
                role: "user".to_string(),
                content: vec![
                    ClaudeContentBlock::Text {
                        text: doc_prompt,
                        cache_control: Some(CacheControl {
                            r#type: "ephemeral".to_string(),
                        }),
                    },
                    ClaudeContentBlock::Text {
                        text: chunk_prompt,
                        cache_control: None,
                    },
                ],
            }],
            temperature: Some(0.0),
        };

        let response = self
            .client
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "prompt-caching-2024-07-31")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Llm(format!("Claude request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Llm(format!("Claude returned {status}: {body}")));
        }

        let result: ClaudeResponse = response
            .json()
            .await
            .map_err(|e| Error::Llm(format!("Failed to parse Claude response: {e}")))?;

        debug!(
            model = %result.model,
            input_tokens = result.usage.input_tokens,
            cache_read = result.usage.cache_read_input_tokens.unwrap_or(0),
            "Claude contextualization completed"
        );

        result
            .content
            .first()
            .and_then(|block| match block {
                ClaudeResponseBlock::Text { text } => Some(text.clone()),
            })
            .ok_or_else(|| Error::Llm("Empty Claude response".to_string()))
    }
}

// --- Claude API types ---

#[derive(Serialize)]
struct ClaudeRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<ClaudeMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize)]
struct ClaudeMessage {
    role: String,
    content: Vec<ClaudeContentBlock>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClaudeContentBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

#[derive(Serialize)]
struct CacheControl {
    r#type: String,
}

#[derive(Deserialize)]
struct ClaudeResponse {
    content: Vec<ClaudeResponseBlock>,
    model: String,
    usage: ClaudeUsage,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClaudeResponseBlock {
    Text { text: String },
}

#[derive(Deserialize)]
#[allow(dead_code)] // Fields populated from API response, only some are read in logs.
struct ClaudeUsage {
    input_tokens: u32,
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompts_contain_placeholders() {
        let doc = DOCUMENT_CONTEXT_PROMPT.replace("{doc_content}", "test doc");
        assert!(doc.contains("test doc"));
        assert!(doc.contains("<document>"));

        let chunk = CHUNK_CONTEXT_PROMPT.replace("{chunk_content}", "test chunk");
        assert!(chunk.contains("test chunk"));
        assert!(chunk.contains("<chunk>"));
    }

    #[test]
    fn test_claude_request_serialization() {
        let request = ClaudeRequest {
            model: "claude-haiku-4-5".to_string(),
            max_tokens: 1024,
            messages: vec![ClaudeMessage {
                role: "user".to_string(),
                content: vec![
                    ClaudeContentBlock::Text {
                        text: "doc content".to_string(),
                        cache_control: Some(CacheControl {
                            r#type: "ephemeral".to_string(),
                        }),
                    },
                    ClaudeContentBlock::Text {
                        text: "chunk content".to_string(),
                        cache_control: None,
                    },
                ],
            }],
            temperature: Some(0.0),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("claude-haiku-4-5"));
        assert!(json.contains("ephemeral"));
        assert!(json.contains("doc content"));
    }
}
