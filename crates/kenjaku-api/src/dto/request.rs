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
    /// Client session identifier.
    pub session_id: String,
    /// Unique request identifier.
    pub request_id: String,
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
    /// Client session identifier.
    pub session_id: String,
    /// Request identifier the feedback is for.
    pub request_id: String,
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
        assert_eq!(dto.session_id, "s");
        assert_eq!(dto.request_id, "r");
        assert!(!dto.streaming);
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
