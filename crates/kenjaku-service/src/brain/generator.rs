//! `GeminiBrain` — the default `Brain` implementation wrapping an
//! `Arc<dyn LlmProvider>`.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use kenjaku_core::error::Result;
use kenjaku_core::traits::brain::Brain;
use kenjaku_core::traits::intent::IntentClassifier;
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::types::intent::IntentClassification;
use kenjaku_core::types::locale::Locale;
use kenjaku_core::types::message::Message;
use kenjaku_core::types::search::{LlmResponse, RetrievedChunk, StreamChunk, TranslationResult};

/// Default `Brain` implementation backed by a single `LlmProvider`.
///
/// Owns no prompt templates — those live in `brain::prompt`. The Brain
/// composes messages via `ConversationAssembler` and delegates all LLM
/// I/O to the injected provider.
pub struct GeminiBrain {
    llm: Arc<dyn LlmProvider>,
    intent_classifier: Arc<dyn IntentClassifier>,
}

impl GeminiBrain {
    pub fn new(llm: Arc<dyn LlmProvider>, intent_classifier: Arc<dyn IntentClassifier>) -> Self {
        Self {
            llm,
            intent_classifier,
        }
    }
}

#[async_trait]
impl Brain for GeminiBrain {
    #[instrument(skip(self, _cancel), fields(query = %query))]
    async fn classify_intent(
        &self,
        query: &str,
        _cancel: &CancellationToken,
    ) -> Result<IntentClassification> {
        self.intent_classifier.classify(query).await
    }

    #[instrument(skip(self, _cancel))]
    async fn translate(
        &self,
        query: &str,
        _cancel: &CancellationToken,
    ) -> Result<TranslationResult> {
        self.llm.translate(query).await
    }

    #[instrument(skip(self, messages, chunks, _cancel), fields(locale = %locale, msg_count = messages.len(), chunk_count = chunks.len()))]
    async fn generate(
        &self,
        messages: &[Message],
        chunks: &[RetrievedChunk],
        locale: Locale,
        _cancel: &CancellationToken,
    ) -> Result<LlmResponse> {
        // Extract the search query from the last user message for the
        // LlmProvider interface. The messages already contain the fully
        // assembled prompt with context.
        //
        // The current LlmProvider::generate signature takes
        // (query, context, history, locale). We pass the raw query
        // extracted from the last user message, the chunks, an empty
        // history (history is already baked into messages — but the
        // current LlmProvider still rebuilds its own multi-turn from
        // the ConversationTurn slice), and the locale.
        //
        // TODO: Phase 3b — change LlmProvider to accept &[Message]
        // directly. For now we extract back to the old interface to
        // avoid changing the trait signature in this PR.
        let (query, history) = extract_query_and_history(messages);
        self.llm.generate(&query, chunks, &history, locale).await
    }

    #[instrument(skip(self, messages, chunks, _cancel), fields(locale = %locale, msg_count = messages.len()))]
    async fn generate_stream(
        &self,
        messages: &[Message],
        chunks: &[RetrievedChunk],
        locale: Locale,
        _cancel: &CancellationToken,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        let (query, history) = extract_query_and_history(messages);
        self.llm
            .generate_stream(&query, chunks, &history, locale)
            .await
    }

    #[instrument(skip(self, _cancel))]
    async fn suggest(
        &self,
        query: &str,
        answer: &str,
        _cancel: &CancellationToken,
    ) -> Result<Vec<String>> {
        self.llm.suggest(query, answer).await
    }
}

/// Extract the search query string and conversation history from a
/// `Vec<Message>` assembled by `ConversationAssembler`.
///
/// The message sequence is: System, [User, Assistant]*, User (final).
/// We need to pass the final user turn's text as `query` and rebuild
/// `ConversationTurn` pairs from the intermediate User/Assistant pairs.
///
/// This is a temporary bridge until `LlmProvider` accepts `&[Message]`
/// directly.
fn extract_query_and_history(
    messages: &[Message],
) -> (
    String,
    Vec<kenjaku_core::types::conversation::ConversationTurn>,
) {
    use kenjaku_core::types::message::{ContentPart, Role};

    let mut history = Vec::new();
    let mut query = String::new();

    // Skip the system message (index 0), then process pairs.
    let body = if !messages.is_empty() && messages[0].role == Role::System {
        &messages[1..]
    } else {
        messages
    };

    if body.is_empty() {
        return (query, history);
    }

    // Last message is the current user query
    let last = &body[body.len() - 1];
    if let Some(ContentPart::Text(t)) = last.parts.first() {
        query = t.clone();
    }

    // Everything before the last message is history pairs
    let pairs = &body[..body.len() - 1];
    for chunk in pairs.chunks(2) {
        if chunk.len() == 2 {
            let user_text = chunk[0]
                .parts
                .first()
                .map(|p| match p {
                    ContentPart::Text(t) => t.clone(),
                })
                .unwrap_or_default();
            let asst_text = chunk[1]
                .parts
                .first()
                .map(|p| match p {
                    ContentPart::Text(t) => t.clone(),
                })
                .unwrap_or_default();
            history.push(kenjaku_core::types::conversation::ConversationTurn {
                user: user_text,
                assistant: asst_text,
            });
        }
    }

    (query, history)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenjaku_core::types::message::Message;

    #[test]
    fn extract_query_and_history_no_history() {
        let messages = vec![
            Message::system_text("sys"),
            Message::user_text("what is BTC"),
        ];
        let (query, history) = extract_query_and_history(&messages);
        assert_eq!(query, "what is BTC");
        assert!(history.is_empty());
    }

    #[test]
    fn extract_query_and_history_with_history() {
        let messages = vec![
            Message::system_text("sys"),
            Message::user_text("first question"),
            Message::assistant_text("first answer"),
            Message::user_text("follow up"),
        ];
        let (query, history) = extract_query_and_history(&messages);
        assert_eq!(query, "follow up");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].user, "first question");
        assert_eq!(history[0].assistant, "first answer");
    }

    #[test]
    fn extract_query_and_history_multiple_turns() {
        let messages = vec![
            Message::system_text("sys"),
            Message::user_text("q1"),
            Message::assistant_text("a1"),
            Message::user_text("q2"),
            Message::assistant_text("a2"),
            Message::user_text("q3"),
        ];
        let (query, history) = extract_query_and_history(&messages);
        assert_eq!(query, "q3");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].user, "q1");
        assert_eq!(history[1].user, "q2");
    }

    #[test]
    fn extract_query_and_history_empty() {
        let (query, history) = extract_query_and_history(&[]);
        assert_eq!(query, "");
        assert!(history.is_empty());
    }
}
