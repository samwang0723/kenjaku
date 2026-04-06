use serde::Deserialize;
use utoipa::ToSchema;

use kenjaku_core::types::locale::Locale;

/// Search request body.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SearchRequestDto {
    /// The search query text.
    pub query: String,
    /// Locale/language code. Supported: en, zh, zh-TW, ja, ko, de, fr, es.
    #[serde(default = "default_locale")]
    pub locale: String,
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

fn default_locale() -> String {
    "en".to_string()
}

fn default_top_k() -> Option<usize> {
    Some(10)
}

impl SearchRequestDto {
    /// Parse and validate the locale string into a typed `Locale`.
    pub fn parse_locale(&self) -> Result<Locale, kenjaku_core::error::Error> {
        self.locale.parse()
    }
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
    fn test_parse_locale_valid() {
        let dto = SearchRequestDto {
            query: "test".into(),
            locale: "ja".into(),
            session_id: "s".into(),
            request_id: "r".into(),
            streaming: false,
            top_k: None,
        };
        assert_eq!(dto.parse_locale().unwrap(), Locale::Ja);
    }

    #[test]
    fn test_parse_locale_zh_tw() {
        let dto = SearchRequestDto {
            query: "test".into(),
            locale: "zh-TW".into(),
            session_id: "s".into(),
            request_id: "r".into(),
            streaming: false,
            top_k: None,
        };
        assert_eq!(dto.parse_locale().unwrap(), Locale::ZhTw);
    }

    #[test]
    fn test_parse_locale_invalid() {
        let dto = SearchRequestDto {
            query: "test".into(),
            locale: "pt".into(),
            session_id: "s".into(),
            request_id: "r".into(),
            streaming: false,
            top_k: None,
        };
        assert!(dto.parse_locale().is_err());
    }
}
