use serde::{Deserialize, Serialize};

use super::search::LlmSource;

/// A component in the search response layout.
/// The order of components is configurable via `ComponentLayout`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Component {
    LlmAnswer(LlmAnswerComponent),
    Sources(SourcesComponent),
    Suggestions(SuggestionsComponent),
}

/// The LLM-generated answer component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmAnswerComponent {
    pub answer: String,
    pub model: String,
}

/// Sources referenced in the answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcesComponent {
    pub sources: Vec<LlmSource>,
}

/// Follow-up query suggestions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionsComponent {
    pub suggestions: Vec<String>,
    pub source: SuggestionSource,
}

/// Where suggestions came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SuggestionSource {
    Llm,
    VectorStore,
}

/// Defines which components appear and in what order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentLayout {
    pub order: Vec<ComponentType>,
}

/// Identifiers for component types used in layout configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ComponentType {
    LlmAnswer,
    Sources,
    Suggestions,
}

impl Default for ComponentLayout {
    fn default() -> Self {
        Self {
            order: vec![
                ComponentType::LlmAnswer,
                ComponentType::Sources,
                ComponentType::Suggestions,
            ],
        }
    }
}
