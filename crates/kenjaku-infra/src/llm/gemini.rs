use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::instrument;

use kenjaku_core::config::LlmConfig;
use kenjaku_core::error::{Error, Result};
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::types::search::{
    LlmResponse, LlmSource, LlmUsage, RetrievedChunk, StreamChunk, StreamChunkType,
};

/// Gemini LLM provider implementation.
pub struct GeminiProvider {
    client: Client,
    config: LlmConfig,
    base_url: String,
}

impl GeminiProvider {
    pub fn new(config: LlmConfig) -> Self {
        Self {
            client: Client::new(),
            base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
            config,
        }
    }

    /// Build the context string from retrieved chunks.
    fn build_context(chunks: &[RetrievedChunk]) -> String {
        chunks
            .iter()
            .enumerate()
            .map(|(i, chunk)| {
                format!(
                    "[Source {}] {}\n{}\n",
                    i + 1,
                    chunk.title,
                    chunk.original_content,
                )
            })
            .collect::<Vec<_>>()
            .join("\n---\n")
    }

    /// Build the search prompt with context.
    fn build_search_prompt(query: &str, context: &str) -> String {
        format!(
            "You are a helpful search assistant. Answer the user's question based on the provided context.\n\
            If the context doesn't contain enough information, say so honestly.\n\
            Always cite your sources by referencing [Source N] numbers.\n\n\
            Context:\n{context}\n\n\
            Question: {query}\n\n\
            Answer:"
        )
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    #[instrument(skip(self, context), fields(model = %self.config.model))]
    async fn generate(
        &self,
        query: &str,
        context: &[RetrievedChunk],
    ) -> Result<LlmResponse> {
        let context_str = Self::build_context(context);
        let prompt = Self::build_search_prompt(query, &context_str);

        let request = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![GeminiPart::Text { text: prompt }],
                role: Some("user".to_string()),
            }],
            tools: Some(vec![GeminiTool {
                google_search: Some(serde_json::json!({})),
            }]),
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(self.config.max_tokens),
                temperature: Some(self.config.temperature),
            }),
        };

        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, self.config.model, self.config.api_key
        );

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Llm(format!("Gemini request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Llm(format!("Gemini returned {status}: {body}")));
        }

        let result: GeminiResponse = response
            .json()
            .await
            .map_err(|e| Error::Llm(format!("Failed to parse Gemini response: {e}")))?;

        let answer = result
            .candidates
            .first()
            .and_then(|c| c.content.parts.first())
            .map(|p| match p {
                GeminiPart::Text { text } => text.clone(),
            })
            .unwrap_or_default();

        // Extract sources from grounding metadata if available
        let sources = result
            .candidates
            .first()
            .and_then(|c| c.grounding_metadata.as_ref())
            .map(|meta| {
                meta.grounding_chunks
                    .iter()
                    .filter_map(|chunk| {
                        chunk.web.as_ref().map(|w| LlmSource {
                            title: w.title.clone().unwrap_or_default(),
                            url: w.uri.clone().unwrap_or_default(),
                            snippet: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let usage = result.usage_metadata.map(|u| LlmUsage {
            prompt_tokens: u.prompt_token_count.unwrap_or(0),
            completion_tokens: u.candidates_token_count.unwrap_or(0),
            total_tokens: u.total_token_count.unwrap_or(0),
        });

        Ok(LlmResponse {
            answer,
            sources,
            model: self.config.model.clone(),
            usage,
        })
    }

    async fn generate_stream(
        &self,
        query: &str,
        context: &[RetrievedChunk],
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        let context_str = Self::build_context(context);
        let prompt = Self::build_search_prompt(query, &context_str);

        let request = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![GeminiPart::Text { text: prompt }],
                role: Some("user".to_string()),
            }],
            tools: Some(vec![GeminiTool {
                google_search: Some(serde_json::json!({})),
            }]),
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(self.config.max_tokens),
                temperature: Some(self.config.temperature),
            }),
        };

        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url, self.config.model, self.config.api_key
        );

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Llm(format!("Gemini streaming request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Llm(format!("Gemini returned {status}: {body}")));
        }

        let byte_stream = response.bytes_stream();
        let stream = futures::stream::unfold(
            (byte_stream, String::new()),
            |(mut byte_stream, mut buffer)| async move {
                use futures::StreamExt;
                loop {
                    match byte_stream.next().await {
                        Some(Ok(bytes)) => {
                            buffer.push_str(&String::from_utf8_lossy(&bytes));

                            // Parse SSE data lines
                            while let Some(pos) = buffer.find("\n\n") {
                                let event = buffer[..pos].to_string();
                                buffer = buffer[pos + 2..].to_string();

                                if let Some(data) = event.strip_prefix("data: ") {
                                    if let Ok(response) =
                                        serde_json::from_str::<GeminiResponse>(data)
                                    {
                                        if let Some(text) = response
                                            .candidates
                                            .first()
                                            .and_then(|c| c.content.parts.first())
                                            .map(|p| match p {
                                                GeminiPart::Text { text } => text.clone(),
                                            })
                                        {
                                            let finished = response
                                                .candidates
                                                .first()
                                                .and_then(|c| c.finish_reason.as_ref())
                                                .is_some();

                                            return Some((
                                                Ok(StreamChunk {
                                                    delta: text,
                                                    chunk_type: StreamChunkType::Answer,
                                                    finished,
                                                }),
                                                (byte_stream, buffer),
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            return Some((
                                Err(Error::Llm(format!("Stream error: {e}"))),
                                (byte_stream, buffer),
                            ));
                        }
                        None => return None,
                    }
                }
            },
        );

        Ok(Box::pin(stream))
    }

    #[instrument(skip(self))]
    async fn translate(
        &self,
        text: &str,
        from_locale: &str,
        to_locale: &str,
    ) -> Result<String> {
        let prompt = format!(
            "Translate the following text from {from_locale} to {to_locale}.\n\
            Pay special attention to domain-specific terminology, technical terms, and proper nouns.\n\
            Keep the meaning precise and natural in {to_locale}.\n\
            Return ONLY the translation, nothing else.\n\n\
            Text: {text}"
        );

        let request = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![GeminiPart::Text { text: prompt }],
                role: Some("user".to_string()),
            }],
            tools: None,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(1024),
                temperature: Some(0.1),
            }),
        };

        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, self.config.model, self.config.api_key
        );

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Translation(format!("Gemini translation failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Translation(format!(
                "Gemini returned {status}: {body}"
            )));
        }

        let result: GeminiResponse = response
            .json()
            .await
            .map_err(|e| Error::Translation(format!("Failed to parse translation: {e}")))?;

        result
            .candidates
            .first()
            .and_then(|c| c.content.parts.first())
            .map(|p| match p {
                GeminiPart::Text { text } => text.clone(),
            })
            .ok_or_else(|| Error::Translation("Empty translation response".to_string()))
    }

    #[instrument(skip(self))]
    async fn suggest(
        &self,
        query: &str,
        answer: &str,
    ) -> Result<Vec<String>> {
        let prompt = format!(
            "Based on the following question and answer, suggest exactly 3 follow-up questions \
            the user might want to ask. Return them as a JSON array of strings.\n\n\
            Question: {query}\n\
            Answer: {answer}\n\n\
            Return ONLY a JSON array like: [\"question 1\", \"question 2\", \"question 3\"]"
        );

        let request = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![GeminiPart::Text { text: prompt }],
                role: Some("user".to_string()),
            }],
            tools: None,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(512),
                temperature: Some(0.8),
            }),
        };

        let url = format!(
            "{}/models/{}:generateContent?key={}",
            self.base_url, self.config.model, self.config.api_key
        );

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| Error::Llm(format!("Gemini suggest failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Llm(format!("Gemini returned {status}: {body}")));
        }

        let result: GeminiResponse = response
            .json()
            .await
            .map_err(|e| Error::Llm(format!("Failed to parse suggestions: {e}")))?;

        let text = result
            .candidates
            .first()
            .and_then(|c| c.content.parts.first())
            .map(|p| match p {
                GeminiPart::Text { text } => text.clone(),
            })
            .unwrap_or_default();

        // Parse JSON array from response
        serde_json::from_str::<Vec<String>>(&text).or_else(|_| {
            // Try to extract JSON array from markdown code block
            let trimmed = text
                .trim()
                .strip_prefix("```json")
                .or_else(|| text.trim().strip_prefix("```"))
                .unwrap_or(&text)
                .strip_suffix("```")
                .unwrap_or(&text)
                .trim();
            serde_json::from_str::<Vec<String>>(trimmed)
                .map_err(|e| Error::Llm(format!("Failed to parse suggestions JSON: {e}")))
        })
    }
}

// --- Gemini API types ---

#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<GeminiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Serialize, Deserialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum GeminiPart {
    Text { text: String },
}

#[derive(Serialize)]
struct GeminiTool {
    #[serde(skip_serializing_if = "Option::is_none")]
    google_search: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    grounding_metadata: Option<GroundingMetadata>,
}

#[derive(Deserialize)]
struct GroundingMetadata {
    #[serde(default)]
    grounding_chunks: Vec<GroundingChunk>,
}

#[derive(Deserialize)]
struct GroundingChunk {
    #[serde(default)]
    web: Option<WebChunk>,
}

#[derive(Deserialize)]
struct WebChunk {
    uri: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize)]
struct GeminiUsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
}
