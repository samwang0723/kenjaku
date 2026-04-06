use serde::Deserialize;
use utoipa::ToSchema;

/// Search request body.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SearchRequestDto {
    /// The search query text.
    pub query: String,
    /// Locale/language code (e.g., "en", "ja", "zh").
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
