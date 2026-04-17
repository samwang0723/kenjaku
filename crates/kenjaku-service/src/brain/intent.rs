use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tracing::instrument;

use kenjaku_core::error::Result;
use kenjaku_core::prompts;
use kenjaku_core::traits::intent::IntentClassifier;
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_core::types::intent::{Intent, IntentClassification};
use kenjaku_core::types::search::LlmResponse;
use kenjaku_core::types::usage::LlmCall;

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
    async fn classify(&self, query: &str) -> Result<(IntentClassification, Option<LlmCall>)> {
        // Template lives in `crates/kenjaku-core/src/prompts/classify_intent.md`.
        // The query is sandboxed in `<query>...</query>` tags so instruction
        // injection via the user string is contained.
        let prompt = prompts::render(prompts::CLASSIFY_INTENT, &[("query", query)]);

        // Intent classifier doesn't care about answer language — pass `En`
        // as a no-op; the empty-context branch in `GeminiProvider::generate`
        // skips the systemInstruction entirely anyway.
        let started = Instant::now();
        let response: LlmResponse = self.llm.generate_brief(&prompt).await?;
        let latency_ms = started.elapsed().as_millis() as u64;

        let call = response.usage.as_ref().map(|u| LlmCall {
            purpose: "classify_intent".to_string(),
            model: response.model.clone(),
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            cost_usd: u.cost_usd.unwrap_or(0.0),
            latency_ms,
        });

        let intent = Intent::from_raw(&response.answer);

        Ok((
            IntentClassification {
                intent,
                confidence: if intent == Intent::Unknown { 0.0 } else { 0.85 },
            },
            call,
        ))
    }
}

// Intent parsing moved to `Intent::from_raw` in kenjaku-core so the
// merged-preamble path in `GeminiProvider::preprocess_query` can use
// the same logic. See `crates/kenjaku-core/src/types/intent.rs`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_intent_valid() {
        assert_eq!(Intent::from_raw("factual"), Intent::Factual);
        assert_eq!(Intent::from_raw("navigational"), Intent::Navigational);
        assert_eq!(Intent::from_raw("how_to"), Intent::HowTo);
        assert_eq!(Intent::from_raw("howto"), Intent::HowTo);
        assert_eq!(Intent::from_raw("how-to"), Intent::HowTo);
        assert_eq!(Intent::from_raw("comparison"), Intent::Comparison);
        assert_eq!(Intent::from_raw("troubleshooting"), Intent::Troubleshooting);
        assert_eq!(Intent::from_raw("exploratory"), Intent::Exploratory);
        assert_eq!(Intent::from_raw("conversational"), Intent::Conversational);
    }

    #[test]
    fn test_parse_intent_unknown() {
        assert_eq!(Intent::from_raw(""), Intent::Unknown);
        assert_eq!(Intent::from_raw("garbage"), Intent::Unknown);
    }

    #[test]
    fn test_parse_intent_case_insensitive() {
        // Promoted helper now lowercases before matching, so uppercase
        // and mixed-case inputs are accepted (LLMs occasionally emit
        // these even when prompted for snake_case).
        assert_eq!(Intent::from_raw("FACTUAL"), Intent::Factual);
        assert_eq!(Intent::from_raw("How_To"), Intent::HowTo);
        assert_eq!(
            Intent::from_raw("  conversational  "),
            Intent::Conversational
        );
    }
}
