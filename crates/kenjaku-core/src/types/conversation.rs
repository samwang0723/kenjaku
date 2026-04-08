use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::intent::Intent;
use super::locale::Locale;

/// A stored conversation record for analytics and audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: Uuid,
    pub session_id: String,
    pub request_id: String,
    pub query: String,
    pub response_text: String,
    pub locale: Locale,
    pub intent: Intent,
    pub meta: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// A single user↔assistant exchange held in the in-memory
/// `SessionHistoryStore` to give the LLM follow-up context. This is NOT
/// the durable record — the Postgres `conversations` table is still the
/// source of truth for analytics; this struct only lives for the lifetime
/// of a session's runtime memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub user: String,
    pub assistant: String,
}

/// Request to create a conversation record (used by the flush pipeline).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateConversation {
    pub session_id: String,
    pub request_id: String,
    pub query: String,
    pub response_text: String,
    pub locale: Locale,
    pub intent: Intent,
    pub meta: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_conversation() {
        let conv = CreateConversation {
            session_id: "sess-1".to_string(),
            request_id: "req-1".to_string(),
            query: "What is Rust?".to_string(),
            response_text: "Rust is a systems programming language.".to_string(),
            locale: Locale::En,
            intent: Intent::Factual,
            meta: serde_json::json!({"model": "gemini-2.0-flash-lite"}),
        };
        assert_eq!(conv.locale, Locale::En);
        assert_eq!(conv.intent, Intent::Factual);
    }

    #[test]
    fn test_conversation_serde() {
        let conv = Conversation {
            id: Uuid::new_v4(),
            session_id: "sess-1".to_string(),
            request_id: "req-1".to_string(),
            query: "test".to_string(),
            response_text: "answer".to_string(),
            locale: Locale::Ja,
            intent: Intent::HowTo,
            meta: serde_json::json!({}),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&conv).unwrap();
        let parsed: Conversation = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.session_id, "sess-1");
        assert_eq!(parsed.locale, Locale::Ja);
        assert_eq!(parsed.intent, Intent::HowTo);
    }
}
