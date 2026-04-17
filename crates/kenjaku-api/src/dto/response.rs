use serde::Serialize;
use utoipa::ToSchema;

use kenjaku_core::types::component::Component;
use kenjaku_core::types::search::SearchResponse;
use kenjaku_core::types::usage::{LlmCall, UsageStats};

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
    Assets {
        assets: Vec<AssetDto>,
    },
}

/// Financial asset reference (stock or crypto ticker) extracted from
/// the answer's merged-JSON output. Wire mirror of
/// [`kenjaku_core::types::assets::Asset`].
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AssetDto {
    pub symbol: String,
    /// One of `"stock"` or `"crypto"`.
    #[serde(rename = "type")]
    pub asset_type: String,
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
    /// Resolved hot-path locale (post-resolution chain: query_param /
    /// session_memory / accept_language / default). Additive, non-breaking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_locale: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_locale_source: Option<String>,
    pub intent: String,
    pub retrieval_count: usize,
    pub latency_ms: u64,
    /// Web tier provenance — populated when SearchService called the
    /// configured web search backend (Brave, etc.) for this request.
    /// Default-skipped on the wire when both flags are false.
    #[serde(default)]
    pub grounding: GroundingInfoDto,
    /// Per-request LLM token usage + estimated cost. One entry per
    /// LLM call (translator, classifier, generator, suggest). Operators
    /// can use this to track per-request cost and spot expensive
    /// queries without parsing logs.
    #[serde(default)]
    pub usage: UsageStatsDto,
}

/// Per-request LLM usage aggregate mirrored from
/// [`kenjaku_core::types::usage::UsageStats`] for the wire.
#[derive(Debug, Default, Serialize, ToSchema)]
pub struct UsageStatsDto {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
    pub estimated_cost_usd: f64,
    pub calls: Vec<LlmCallDto>,
}

/// Single LLM call accounting entry mirrored from
/// [`kenjaku_core::types::usage::LlmCall`] for the wire.
#[derive(Debug, Serialize, ToSchema)]
pub struct LlmCallDto {
    pub purpose: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: f64,
    pub latency_ms: u64,
}

impl From<UsageStats> for UsageStatsDto {
    fn from(u: UsageStats) -> Self {
        Self {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            total_tokens: u.total_tokens,
            estimated_cost_usd: u.estimated_cost_usd,
            calls: u.calls.into_iter().map(LlmCallDto::from).collect(),
        }
    }
}

impl From<LlmCall> for LlmCallDto {
    fn from(c: LlmCall) -> Self {
        Self {
            purpose: c.purpose,
            model: c.model,
            input_tokens: c.input_tokens,
            output_tokens: c.output_tokens,
            cost_usd: c.cost_usd,
            latency_ms: c.latency_ms,
        }
    }
}

/// Mirrors `kenjaku_core::types::search::GroundingInfo` for the wire.
#[derive(Debug, Default, Serialize, ToSchema)]
pub struct GroundingInfoDto {
    #[serde(default)]
    pub web_search_used: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search_provider: Option<String>,
    #[serde(default)]
    pub web_search_count: usize,
    #[serde(default)]
    pub gemini_grounding_used: bool,
}

/// A single blended suggestion row — matches the domain `BlendedSuggestion`
/// produced by dev-1's `SuggestionService`. Crowdsourced rows carry a `score`
/// (search_count); default rows carry a `weight`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct BlendedItemDto {
    pub query: String,
    /// `"crowdsourced"` or `"default"`.
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<i32>,
}

/// Top searches response — gains `resolved_locale` / `resolved_locale_source`
/// per the locked DTO contract in `architect.md`.
#[derive(Debug, Serialize, ToSchema)]
pub struct TopSearchesResponse {
    pub items: Vec<BlendedItemDto>,
    pub resolved_locale: String,
    pub resolved_locale_source: String,
}

/// Legacy per-item shape still used internally by some callers; retained for
/// backwards compatibility with the existing SSE / client contract. New code
/// should prefer [`BlendedItemDto`].
#[derive(Debug, Serialize, ToSchema)]
pub struct TopSearchDto {
    pub query: String,
    pub count: i64,
}

/// Autocomplete response — now carries blended items and the resolved locale
/// fields so the client can mirror the server's resolution decision.
#[derive(Debug, Serialize, ToSchema)]
pub struct AutocompleteResponseDto {
    pub suggestions: Vec<String>,
    pub items: Vec<BlendedItemDto>,
    pub resolved_locale: String,
    pub resolved_locale_source: String,
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
                resolved_locale: None,
                resolved_locale_source: None,
                intent: resp.metadata.intent.to_string(),
                retrieval_count: resp.metadata.retrieval_count,
                latency_ms: resp.metadata.latency_ms,
                grounding: GroundingInfoDto {
                    web_search_used: resp.metadata.grounding.web_search_used,
                    web_search_provider: resp.metadata.grounding.web_search_provider.clone(),
                    web_search_count: resp.metadata.grounding.web_search_count,
                    gemini_grounding_used: resp.metadata.grounding.gemini_grounding_used,
                },
                usage: resp.metadata.usage.into(),
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
            Component::Assets(a) => Self::Assets {
                assets: a
                    .assets
                    .into_iter()
                    .map(|asset| AssetDto {
                        symbol: asset.symbol,
                        asset_type: asset.asset_type.as_str().to_string(),
                    })
                    .collect(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_metadata_serializes_with_resolved_locale_fields() {
        let meta = SearchMetadataDto {
            original_query: "hi".into(),
            translated_query: None,
            locale: "ja".into(),
            detected_locale_source: "llm_detected".into(),
            resolved_locale: Some("ja".into()),
            resolved_locale_source: Some("session_memory".into()),
            intent: "unknown".into(),
            retrieval_count: 3,
            latency_ms: 120,
            grounding: GroundingInfoDto::default(),
            usage: UsageStatsDto::default(),
        };
        let v = serde_json::to_value(&meta).unwrap();
        assert_eq!(v.get("resolved_locale").unwrap(), "ja");
        assert_eq!(v.get("resolved_locale_source").unwrap(), "session_memory");
        assert_eq!(v.get("locale").unwrap(), "ja");
    }

    #[test]
    fn search_metadata_omits_resolved_fields_when_none() {
        let meta = SearchMetadataDto {
            original_query: "hi".into(),
            translated_query: None,
            locale: "en".into(),
            detected_locale_source: "llm_detected".into(),
            resolved_locale: None,
            resolved_locale_source: None,
            intent: "unknown".into(),
            retrieval_count: 0,
            latency_ms: 10,
            grounding: GroundingInfoDto::default(),
            usage: UsageStatsDto::default(),
        };
        let v = serde_json::to_value(&meta).unwrap();
        assert!(v.get("resolved_locale").is_none());
        assert!(v.get("resolved_locale_source").is_none());
    }

    #[test]
    fn search_metadata_includes_usage_payload() {
        let meta = SearchMetadataDto {
            original_query: "hi".into(),
            translated_query: None,
            locale: "en".into(),
            detected_locale_source: "llm_detected".into(),
            resolved_locale: None,
            resolved_locale_source: None,
            intent: "factual".into(),
            retrieval_count: 2,
            latency_ms: 200,
            grounding: GroundingInfoDto::default(),
            usage: UsageStatsDto {
                input_tokens: 150,
                output_tokens: 60,
                total_tokens: 210,
                estimated_cost_usd: 0.0015,
                calls: vec![LlmCallDto {
                    purpose: "translate".into(),
                    model: "gemini-test".into(),
                    input_tokens: 50,
                    output_tokens: 10,
                    cost_usd: 0.0003,
                    latency_ms: 40,
                }],
            },
        };
        let v = serde_json::to_value(&meta).unwrap();
        let usage = v.get("usage").expect("usage field present");
        assert_eq!(usage.get("total_tokens").unwrap(), 210);
        assert_eq!(usage.get("input_tokens").unwrap(), 150);
        assert_eq!(usage.get("output_tokens").unwrap(), 60);
        let calls = usage.get("calls").unwrap().as_array().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].get("purpose").unwrap(), "translate");
    }

    #[test]
    fn blended_item_dto_round_trips() {
        let crowd = BlendedItemDto {
            query: "why is btc valuable".into(),
            source: "crowdsourced".into(),
            score: Some(42.0),
            weight: None,
        };
        let v = serde_json::to_value(&crowd).unwrap();
        assert_eq!(v.get("score").unwrap(), 42.0);
        assert!(v.get("weight").is_none());

        let default_row = BlendedItemDto {
            query: "How does Level Up work?".into(),
            source: "default".into(),
            score: None,
            weight: Some(10),
        };
        let v = serde_json::to_value(&default_row).unwrap();
        assert_eq!(v.get("weight").unwrap(), 10);
        assert!(v.get("score").is_none());
    }

    #[test]
    fn top_searches_response_shape() {
        let r = TopSearchesResponse {
            items: vec![BlendedItemDto {
                query: "q".into(),
                source: "default".into(),
                score: None,
                weight: Some(10),
            }],
            resolved_locale: "ja".into(),
            resolved_locale_source: "query_param".into(),
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v.get("resolved_locale").unwrap(), "ja");
        assert_eq!(v.get("resolved_locale_source").unwrap(), "query_param");
        assert_eq!(v.get("items").unwrap().as_array().unwrap().len(), 1);
    }
}
