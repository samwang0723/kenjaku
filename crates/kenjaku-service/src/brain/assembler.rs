//! Conversation assembler: builds `Vec<Message>` from history, query, and
//! retrieved context. Pure function, no I/O.

use kenjaku_core::types::conversation::ConversationTurn;
use kenjaku_core::types::locale::Locale;
use kenjaku_core::types::message::Message;
use kenjaku_core::types::search::RetrievedChunk;

use super::prompt::{build_context, build_search_prompt, build_search_system_instruction};

/// Stateless assembler that converts conversation history, query, and
/// retrieved chunks into a `Vec<Message>` ready for any LLM provider.
pub struct ConversationAssembler;

impl ConversationAssembler {
    /// Build the full message sequence for a search request.
    ///
    /// The output order is:
    /// 1. System message (system instruction text)
    /// 2. For each history turn: User message, then Assistant message
    /// 3. Final User message (context + query + locale reminder)
    ///
    /// This is a pure function — cheap, no I/O, cancel-aware only at the
    /// spawn boundary.
    pub fn build(
        history: &[ConversationTurn],
        query: &str,
        locale: Locale,
        has_builtin_web_tool: bool,
        chunks: &[RetrievedChunk],
    ) -> Vec<Message> {
        let system_text = build_search_system_instruction(locale, has_builtin_web_tool);
        let context_str = build_context(chunks);
        let user_turn = build_search_prompt(query, &context_str, locale);

        let mut msgs = Vec::with_capacity(history.len() * 2 + 2);
        msgs.push(Message::system_text(system_text));
        for turn in history {
            msgs.push(Message::user_text(turn.user.clone()));
            msgs.push(Message::assistant_text(turn.assistant.clone()));
        }
        msgs.push(Message::user_text(user_turn));
        msgs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenjaku_core::types::message::{ContentPart, Role};
    use kenjaku_core::types::search::RetrievalMethod;

    fn make_chunk(title: &str, content: &str) -> RetrievedChunk {
        RetrievedChunk {
            doc_id: "d1".into(),
            chunk_id: "c1".into(),
            title: title.into(),
            original_content: content.into(),
            contextualized_content: String::new(),
            source_url: None,
            score: 0.9,
            retrieval_method: RetrievalMethod::Vector,
        }
    }

    fn text_of(msg: &Message) -> &str {
        match &msg.parts[0] {
            ContentPart::Text(s) => s,
        }
    }

    #[test]
    fn assembler_builds_correct_sequence_no_history() {
        let chunks = vec![make_chunk("Bitcoin", "BTC info")];
        let msgs = ConversationAssembler::build(&[], "what is bitcoin", Locale::En, true, &chunks);

        // System + 1 user = 2 messages
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::System);
        assert_eq!(msgs[1].role, Role::User);
        assert!(text_of(&msgs[0]).contains("helpful document search assistant"));
        assert!(text_of(&msgs[1]).contains("Question: what is bitcoin"));
        assert!(text_of(&msgs[1]).contains("[Source 1] Bitcoin"));
    }

    #[test]
    fn assembler_builds_correct_sequence_with_history() {
        let history = vec![
            ConversationTurn {
                user: "what is ETH".into(),
                assistant: "ETH is Ethereum.".into(),
            },
            ConversationTurn {
                user: "how to stake".into(),
                assistant: "You can stake on...".into(),
            },
        ];
        let chunks = vec![make_chunk("Staking", "Staking guide")];
        let msgs =
            ConversationAssembler::build(&history, "what rewards", Locale::En, false, &chunks);

        // System + 2*2 history + 1 user = 6
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[0].role, Role::System);
        assert_eq!(msgs[1].role, Role::User);
        assert_eq!(text_of(&msgs[1]), "what is ETH");
        assert_eq!(msgs[2].role, Role::Assistant);
        assert_eq!(text_of(&msgs[2]), "ETH is Ethereum.");
        assert_eq!(msgs[3].role, Role::User);
        assert_eq!(text_of(&msgs[3]), "how to stake");
        assert_eq!(msgs[4].role, Role::Assistant);
        assert_eq!(text_of(&msgs[4]), "You can stake on...");
        assert_eq!(msgs[5].role, Role::User);
        assert!(text_of(&msgs[5]).contains("Question: what rewards"));
        assert!(text_of(&msgs[5]).contains("[Source 1] Staking"));
    }

    #[test]
    fn assembler_empty_chunks_omits_context_block() {
        let msgs = ConversationAssembler::build(&[], "test query", Locale::En, true, &[]);
        let user_text = text_of(&msgs[1]);
        assert!(!user_text.contains("Internal context:"));
        assert!(user_text.starts_with("Question: test query"));
    }

    #[test]
    fn assembler_system_instruction_uses_correct_locale() {
        let msgs = ConversationAssembler::build(&[], "テスト", Locale::Ja, false, &[]);
        let sys_text = text_of(&msgs[0]);
        assert!(sys_text.contains("Write the final answer in 日本語 (BCP-47 `ja`)"));
    }

    #[test]
    fn assembler_web_tool_flag_propagates() {
        let msgs_with = ConversationAssembler::build(&[], "q", Locale::En, true, &[]);
        let msgs_without = ConversationAssembler::build(&[], "q", Locale::En, false, &[]);
        assert!(text_of(&msgs_with[0]).contains("built-in web search capability"));
        assert!(!text_of(&msgs_without[0]).contains("built-in web search capability"));
    }
}
