/// Role in a conversation turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

/// A single part of a message's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentPart {
    Text(String),
    // Future extension points -- additive only:
    // ToolCall { id: String, name: String, args: serde_json::Value },
    // ToolResult { id: String, content: String },
    // Image { url: String, mime: String },
}

/// An LLM-agnostic message. Each LLM provider maps this to its own wire
/// format internally via `messages_to_wire`.
#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<ContentPart>,
}

impl Message {
    pub fn user_text(s: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            parts: vec![ContentPart::Text(s.into())],
        }
    }

    pub fn assistant_text(s: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            parts: vec![ContentPart::Text(s.into())],
        }
    }

    pub fn system_text(s: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            parts: vec![ContentPart::Text(s.into())],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_constructors_set_role() {
        let user = Message::user_text("hello");
        assert_eq!(user.role, Role::User);
        assert_eq!(user.parts.len(), 1);

        let assistant = Message::assistant_text("hi");
        assert_eq!(assistant.role, Role::Assistant);

        let system = Message::system_text("you are helpful");
        assert_eq!(system.role, Role::System);
    }

    #[test]
    fn content_part_text_round_trip() {
        let msg = Message::user_text("test content");
        match &msg.parts[0] {
            ContentPart::Text(s) => assert_eq!(s, "test content"),
        }
    }
}
