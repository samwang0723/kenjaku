use kenjaku_core::types::component::{
    Component, ComponentLayout, ComponentType, LlmAnswerComponent, SourcesComponent,
    SuggestionSource, SuggestionsComponent,
};
use kenjaku_core::types::search::LlmResponse;

/// Service for assembling response components in the configured layout order.
#[derive(Clone)]
pub struct ComponentService {
    layout: ComponentLayout,
}

impl ComponentService {
    pub fn new(layout: ComponentLayout) -> Self {
        Self { layout }
    }

    /// Assemble components in the configured order.
    pub fn assemble(
        &self,
        llm_response: &LlmResponse,
        suggestions: Vec<String>,
        suggestion_source: SuggestionSource,
    ) -> Vec<Component> {
        self.layout
            .order
            .iter()
            .map(|component_type| match component_type {
                ComponentType::LlmAnswer => Component::LlmAnswer(LlmAnswerComponent {
                    answer: llm_response.answer.clone(),
                    model: llm_response.model.clone(),
                }),
                ComponentType::Sources => Component::Sources(SourcesComponent {
                    sources: llm_response.sources.clone(),
                }),
                ComponentType::Suggestions => Component::Suggestions(SuggestionsComponent {
                    suggestions: suggestions.clone(),
                    source: suggestion_source.clone(),
                }),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenjaku_core::types::search::LlmSource;

    #[test]
    fn test_assemble_default_layout() {
        let service = ComponentService::new(ComponentLayout::default());

        let llm_response = LlmResponse {
            answer: "Test answer".to_string(),
            sources: vec![LlmSource {
                title: "Source 1".to_string(),
                url: "https://example.com".to_string(),
                snippet: None,
            }],
            model: "gemini-2.0-flash-lite".to_string(),
            usage: None,
        };

        let suggestions = vec![
            "Follow-up 1".to_string(),
            "Follow-up 2".to_string(),
            "Follow-up 3".to_string(),
        ];

        let components = service.assemble(&llm_response, suggestions, SuggestionSource::Llm);

        assert_eq!(components.len(), 3);
        assert!(matches!(components[0], Component::LlmAnswer(_)));
        assert!(matches!(components[1], Component::Sources(_)));
        assert!(matches!(components[2], Component::Suggestions(_)));
    }

    #[test]
    fn test_assemble_custom_layout() {
        let layout = ComponentLayout {
            order: vec![ComponentType::Suggestions, ComponentType::LlmAnswer],
        };
        let service = ComponentService::new(layout);

        let llm_response = LlmResponse {
            answer: "Answer".to_string(),
            sources: vec![],
            model: "test".to_string(),
            usage: None,
        };

        let components = service.assemble(
            &llm_response,
            vec!["Sug 1".to_string()],
            SuggestionSource::VectorStore,
        );

        assert_eq!(components.len(), 2);
        assert!(matches!(components[0], Component::Suggestions(_)));
        assert!(matches!(components[1], Component::LlmAnswer(_)));
    }
}
