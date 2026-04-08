use serde::Serialize;
use utoipa::ToSchema;

use kenjaku_core::types::component::Component;
use kenjaku_core::types::search::SearchResponse;

/// API response envelope.
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn err(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
        }
    }
}

/// Search response DTO.
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchResponseDto {
    pub request_id: String,
    pub session_id: String,
    pub components: Vec<ComponentDto>,
    pub metadata: SearchMetadataDto,
}

/// A component in the response layout.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ComponentDto {
    LlmAnswer {
        answer: String,
        model: String,
    },
    Sources {
        sources: Vec<SourceDto>,
    },
    Suggestions {
        suggestions: Vec<String>,
        source: String,
    },
}

/// A source reference.
#[derive(Debug, Serialize, ToSchema)]
pub struct SourceDto {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// Search metadata.
#[derive(Debug, Serialize, ToSchema)]
pub struct SearchMetadataDto {
    pub original_query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translated_query: Option<String>,
    /// Detected source locale (LLM auto-detected from the query text).
    pub locale: String,
    /// Provenance of `locale`: `llm_detected` (happy path) or `fallback_en`
    /// (translator failed or returned an unsupported BCP-47 tag).
    pub detected_locale_source: String,
    pub intent: String,
    pub retrieval_count: usize,
    pub latency_ms: u64,
}

/// Top searches response.
#[derive(Debug, Serialize, ToSchema)]
pub struct TopSearchDto {
    pub query: String,
    pub count: i64,
}

/// Autocomplete response.
#[derive(Debug, Serialize, ToSchema)]
pub struct AutocompleteResponseDto {
    pub suggestions: Vec<String>,
}

/// Feedback response.
#[derive(Debug, Serialize, ToSchema)]
pub struct FeedbackResponseDto {
    pub id: String,
    pub session_id: String,
    pub request_id: String,
    pub action: String,
    pub created_at: String,
}

/// Health check response.
#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponseDto {
    pub status: String,
    pub version: String,
}

/// Ready check response.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReadyResponseDto {
    pub status: String,
    pub checks: Vec<ReadyCheckDto>,
}

/// Individual dependency check.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReadyCheckDto {
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<SearchResponse> for SearchResponseDto {
    fn from(resp: SearchResponse) -> Self {
        Self {
            request_id: resp.request_id,
            session_id: resp.session_id,
            components: resp
                .components
                .into_iter()
                .map(ComponentDto::from)
                .collect(),
            metadata: SearchMetadataDto {
                original_query: resp.metadata.original_query,
                translated_query: resp.metadata.translated_query,
                locale: resp.metadata.locale.to_string(),
                detected_locale_source: serde_json::to_value(resp.metadata.detected_locale_source)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
                intent: resp.metadata.intent.to_string(),
                retrieval_count: resp.metadata.retrieval_count,
                latency_ms: resp.metadata.latency_ms,
            },
        }
    }
}

impl From<Component> for ComponentDto {
    fn from(c: Component) -> Self {
        match c {
            Component::LlmAnswer(a) => Self::LlmAnswer {
                answer: a.answer,
                model: a.model,
            },
            Component::Sources(s) => Self::Sources {
                sources: s
                    .sources
                    .into_iter()
                    .map(|src| SourceDto {
                        title: src.title,
                        url: src.url,
                        snippet: src.snippet,
                    })
                    .collect(),
            },
            Component::Suggestions(s) => Self::Suggestions {
                suggestions: s.suggestions,
                source: format!("{:?}", s.source).to_lowercase(),
            },
        }
    }
}
