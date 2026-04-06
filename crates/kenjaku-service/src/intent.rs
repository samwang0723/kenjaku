use std::sync::Arc;

use async_trait::async_trait;
use tracing::instrument;

use kenjaku_core::error::Result;
use kenjaku_core::traits::intent::IntentClassifier;
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::types::intent::{Intent, IntentClassification};
use kenjaku_core::types::search::{LlmResponse, RetrievedChunk};

const INTENT_CLASSIFICATION_PROMPT: &str = r#"Classify the user query into exactly one intent category.

Categories:
- factual: Seeking specific factual information (e.g., "What is the capital of France?")
- navigational: Looking for a specific page or resource (e.g., "login page", "API docs")
- how_to: Procedural or step-by-step question (e.g., "How do I reset my password?")
- comparison: Comparing options (e.g., "Bitcoin vs Ethereum")
- troubleshooting: Diagnosing or fixing a problem (e.g., "Why is my transaction failing?")
- exploratory: Open-ended research (e.g., "Tell me about DeFi")
- conversational: Chitchat not related to search (e.g., "Hello", "Thanks")
- unknown: Cannot determine intent

User query: "{query}"

Respond with ONLY the category name (one word), nothing else."#;

/// LLM-based intent classifier.
pub struct LlmIntentClassifier {
    llm: Arc<dyn LlmProvider>,
}

impl LlmIntentClassifier {
    pub fn new(llm: Arc<dyn LlmProvider>) -> Self {
        Self { llm }
    }
}

#[async_trait]
impl IntentClassifier for LlmIntentClassifier {
    #[instrument(skip(self), fields(query = %query))]
    async fn classify(&self, query: &str) -> Result<IntentClassification> {
        let prompt = INTENT_CLASSIFICATION_PROMPT.replace("{query}", query);

        // Use generate with empty context for a simple classification call
        let response: LlmResponse = self.llm.generate(&prompt, &[] as &[RetrievedChunk]).await?;

        let raw = response.answer.trim().to_lowercase();
        let intent = parse_intent(&raw);

        Ok(IntentClassification {
            intent,
            confidence: if intent == Intent::Unknown { 0.0 } else { 0.85 },
        })
    }
}

fn parse_intent(raw: &str) -> Intent {
    match raw {
        "factual" => Intent::Factual,
        "navigational" => Intent::Navigational,
        "how_to" | "howto" | "how-to" => Intent::HowTo,
        "comparison" => Intent::Comparison,
        "troubleshooting" => Intent::Troubleshooting,
        "exploratory" => Intent::Exploratory,
        "conversational" => Intent::Conversational,
        _ => Intent::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_intent_valid() {
        assert_eq!(parse_intent("factual"), Intent::Factual);
        assert_eq!(parse_intent("navigational"), Intent::Navigational);
        assert_eq!(parse_intent("how_to"), Intent::HowTo);
        assert_eq!(parse_intent("howto"), Intent::HowTo);
        assert_eq!(parse_intent("how-to"), Intent::HowTo);
        assert_eq!(parse_intent("comparison"), Intent::Comparison);
        assert_eq!(parse_intent("troubleshooting"), Intent::Troubleshooting);
        assert_eq!(parse_intent("exploratory"), Intent::Exploratory);
        assert_eq!(parse_intent("conversational"), Intent::Conversational);
    }

    #[test]
    fn test_parse_intent_unknown() {
        assert_eq!(parse_intent(""), Intent::Unknown);
        assert_eq!(parse_intent("garbage"), Intent::Unknown);
        assert_eq!(parse_intent("FACTUAL"), Intent::Unknown); // case sensitive after lowering
    }
}
