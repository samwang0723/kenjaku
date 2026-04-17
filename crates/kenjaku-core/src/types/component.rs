use serde::{Deserialize, Serialize};

use super::assets::Asset;
use super::search::LlmSource;

/// A component in the search response layout.
/// The order of components is configurable via `ComponentLayout`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Component {
    LlmAnswer(LlmAnswerComponent),
    Sources(SourcesComponent),
    Suggestions(SuggestionsComponent),
    /// Extracted financial assets (stock + crypto tickers) the answer
    /// referenced as primary subjects. Populated from the merged
    /// generate call's JSON output; empty block is omitted.
    Assets(AssetsComponent),
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

/// Extracted financial asset references (stocks + crypto).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetsComponent {
    pub assets: Vec<Asset>,
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
    Assets,
}

impl Default for ComponentLayout {
    fn default() -> Self {
        Self {
            order: vec![
                ComponentType::LlmAnswer,
                ComponentType::Assets,
                ComponentType::Sources,
                ComponentType::Suggestions,
            ],
        }
    }
}
