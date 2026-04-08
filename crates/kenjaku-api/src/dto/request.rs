use serde::Deserialize;
use utoipa::ToSchema;

/// Search request body.
///
/// **Note**: There is intentionally no `locale` field. Source language is
/// auto-detected by the LLM translator from the query text and surfaced in
/// the response (`metadata.locale`) and the SSE `start` event.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SearchRequestDto {
    /// The search query text.
    pub query: String,
    /// Client session identifier. Prefer the `X-Session-Id` header; this
    /// body field is kept for backward compatibility and is only used if
    /// the header is absent.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Unique request identifier. Prefer the `X-Request-Id` header; this
    /// body field is kept for backward compatibility and is only used if
    /// the header is absent. If neither is provided the server generates
    /// a UUID.
    #[serde(default)]
    pub request_id: Option<String>,
    /// Whether to stream the response via SSE.
    #[serde(default)]
    pub streaming: bool,
    /// Number of top results to retrieve.
    #[serde(default = "default_top_k")]
    pub top_k: Option<usize>,
}

fn default_top_k() -> Option<usize> {
    Some(10)
}

/// Feedback request body.
#[derive(Debug, Deserialize, ToSchema)]
pub struct FeedbackRequestDto {
    /// Client session identifier. Prefer the `X-Session-Id` header; body
    /// field is optional fallback for backward compatibility.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Request identifier the feedback is for. Prefer the `X-Request-Id`
    /// header; body field is optional fallback.
    #[serde(default)]
    pub request_id: Option<String>,
    /// User action: "like", "dislike", or "cancel".
    pub action: String,
    /// Optional reason category ID.
    pub reason_category_id: Option<i32>,
    /// Optional free-text description.
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_dto_parses_without_locale() {
        let json = r#"{"query":"test","session_id":"s","request_id":"r","streaming":false}"#;
        let dto: SearchRequestDto = serde_json::from_str(json).unwrap();
        assert_eq!(dto.query, "test");
        assert_eq!(dto.session_id.as_deref(), Some("s"));
        assert_eq!(dto.request_id.as_deref(), Some("r"));
        assert!(!dto.streaming);
    }

    #[test]
    fn test_search_dto_parses_without_ids() {
        // Header-first path: handler pulls session/request ids from
        // X-Session-Id / X-Request-Id and the body can omit them.
        let json = r#"{"query":"test","streaming":false}"#;
        let dto: SearchRequestDto = serde_json::from_str(json).unwrap();
        assert_eq!(dto.query, "test");
        assert!(dto.session_id.is_none());
        assert!(dto.request_id.is_none());
    }

    #[test]
    fn test_search_dto_ignores_legacy_locale_field() {
        // serde is lenient by default — unknown fields like a stale
        // `locale` are silently ignored rather than rejected. This keeps
        // legacy clients (still sending the field after its removal from
        // the API) parsing cleanly instead of 4xx-ing.
        let json = r#"{"query":"test","locale":"en","session_id":"s","request_id":"r"}"#;
        let dto: SearchRequestDto = serde_json::from_str(json).unwrap();
        assert_eq!(dto.query, "test");
    }
}
