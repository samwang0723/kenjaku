//! `CompositeBrain` — composes three sub-capabilities (`Classifier`,
//! `Translator`, `Generator`) into a single `Brain` the pipeline can
//! depend on.
//!
//! This is the Phase 2 successor to the monolithic `Brain` impl. It
//! exists so each sub-capability can point at a different provider
//! (e.g. Haiku for classification, Gemini for generation) without
//! touching pipeline code. Phase 2 itself does NOT wire separate
//! providers — all three `Arc<dyn SubTrait>` point at the same
//! `GeminiBrain` instance. The trait infrastructure is what unlocks
//! per-capability routing in Phase 3.
//!
//! See `docs/architecture/flexibility-refactor-tech-spec.md` §3.3.3.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use tokio_util::sync::CancellationToken;

use kenjaku_core::error::Result;
use kenjaku_core::traits::brain::Brain;
use kenjaku_core::traits::classifier::Classifier;
use kenjaku_core::traits::generator::Generator;
use kenjaku_core::traits::translator::Translator;
use kenjaku_core::types::intent::IntentClassification;
use kenjaku_core::types::locale::Locale;
use kenjaku_core::types::message::Message;
use kenjaku_core::types::search::{LlmResponse, RetrievedChunk, StreamChunk, TranslationResult};

/// Composes three independently-swappable sub-capabilities into a
/// single `Brain` the pipeline consumes as `Arc<dyn Brain>`.
///
/// The three `Arc<dyn SubTrait>` fields can point at the same concrete
/// impl (the Phase 2 default — `GeminiBrain` serves all three roles)
/// or at three different providers once Phase 3 lands.
pub struct CompositeBrain {
    pub classifier: Arc<dyn Classifier>,
    pub translator: Arc<dyn Translator>,
    pub generator: Arc<dyn Generator>,
}

impl CompositeBrain {
    pub fn new(
        classifier: Arc<dyn Classifier>,
        translator: Arc<dyn Translator>,
        generator: Arc<dyn Generator>,
    ) -> Self {
        Self {
            classifier,
            translator,
            generator,
        }
    }
}

#[async_trait]
impl Brain for CompositeBrain {
    async fn classify_intent(
        &self,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<IntentClassification> {
        self.classifier.classify(query, cancel).await
    }

    async fn translate(
        &self,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<TranslationResult> {
        self.translator.translate(query, cancel).await
    }

    async fn generate(
        &self,
        messages: &[Message],
        chunks: &[RetrievedChunk],
        locale: Locale,
        cancel: &CancellationToken,
    ) -> Result<LlmResponse> {
        self.generator
            .generate(messages, chunks, locale, cancel)
            .await
    }

    async fn generate_stream(
        &self,
        messages: &[Message],
        chunks: &[RetrievedChunk],
        locale: Locale,
        cancel: &CancellationToken,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        self.generator
            .generate_stream(messages, chunks, locale, cancel)
            .await
    }

    async fn suggest(
        &self,
        query: &str,
        answer: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<String>> {
        self.generator.suggest(query, answer, cancel).await
    }

    fn has_web_grounding(&self) -> bool {
        self.generator.has_web_grounding()
    }

    fn model_name(&self) -> &str {
        self.generator.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use kenjaku_core::types::intent::Intent;
    use kenjaku_core::types::locale::DetectedLocale;
    use kenjaku_core::types::search::StreamChunkType;

    // ---- Mocks with distinct sentinel values to prove delegation --------------

    struct MockClassifier {
        sentinel_confidence: f32,
    }

    #[async_trait]
    impl Classifier for MockClassifier {
        async fn classify(
            &self,
            _query: &str,
            _cancel: &CancellationToken,
        ) -> Result<IntentClassification> {
            Ok(IntentClassification {
                intent: Intent::Navigational,
                confidence: self.sentinel_confidence,
            })
        }
    }

    struct MockTranslator {
        sentinel_normalized: String,
    }

    #[async_trait]
    impl Translator for MockTranslator {
        async fn translate(
            &self,
            _query: &str,
            _cancel: &CancellationToken,
        ) -> Result<TranslationResult> {
            Ok(TranslationResult {
                normalized: self.sentinel_normalized.clone(),
                detected_locale: DetectedLocale::Supported(Locale::Ja),
            })
        }
    }

    struct MockGenerator {
        sentinel_answer: String,
        sentinel_suggestion: String,
        grounding: bool,
        model: String,
    }

    #[async_trait]
    impl Generator for MockGenerator {
        async fn generate(
            &self,
            _messages: &[Message],
            _chunks: &[RetrievedChunk],
            _locale: Locale,
            _cancel: &CancellationToken,
        ) -> Result<LlmResponse> {
            Ok(LlmResponse {
                answer: self.sentinel_answer.clone(),
                sources: vec![],
                model: self.model.clone(),
                usage: None,
            })
        }

        async fn generate_stream(
            &self,
            _messages: &[Message],
            _chunks: &[RetrievedChunk],
            _locale: Locale,
            _cancel: &CancellationToken,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
            let chunk = StreamChunk {
                delta: self.sentinel_answer.clone(),
                chunk_type: StreamChunkType::Answer,
                finished: true,
                grounding: None,
            };
            Ok(Box::pin(stream::iter(vec![Ok(chunk)])))
        }

        async fn suggest(
            &self,
            _query: &str,
            _answer: &str,
            _cancel: &CancellationToken,
        ) -> Result<Vec<String>> {
            Ok(vec![self.sentinel_suggestion.clone()])
        }

        fn has_web_grounding(&self) -> bool {
            self.grounding
        }

        fn model_name(&self) -> &str {
            &self.model
        }
    }

    fn make_composite(
        classifier_conf: f32,
        translator_norm: &str,
        generator_ans: &str,
        generator_sugg: &str,
        grounding: bool,
        model: &str,
    ) -> CompositeBrain {
        CompositeBrain::new(
            Arc::new(MockClassifier {
                sentinel_confidence: classifier_conf,
            }),
            Arc::new(MockTranslator {
                sentinel_normalized: translator_norm.into(),
            }),
            Arc::new(MockGenerator {
                sentinel_answer: generator_ans.into(),
                sentinel_suggestion: generator_sugg.into(),
                grounding,
                model: model.into(),
            }),
        )
    }

    #[tokio::test]
    async fn composite_delegates_classify_to_classifier() {
        let brain = make_composite(0.42, "norm", "ans", "sugg", false, "m");
        let cancel = CancellationToken::new();
        let result = brain.classify_intent("q", &cancel).await.unwrap();
        // The 0.42 confidence proves we hit the MockClassifier, not any
        // other sub-trait mock (they don't produce confidence).
        assert_eq!(result.confidence, 0.42);
        assert_eq!(result.intent, Intent::Navigational);
    }

    #[tokio::test]
    async fn composite_delegates_translate_to_translator() {
        let brain = make_composite(0.0, "TRANSLATOR-SENTINEL", "ans", "sugg", false, "m");
        let cancel = CancellationToken::new();
        let result = brain.translate("q", &cancel).await.unwrap();
        assert_eq!(result.normalized, "TRANSLATOR-SENTINEL");
        match result.detected_locale {
            DetectedLocale::Supported(Locale::Ja) => {}
            other => panic!("translator sentinel locale not returned: {other:?}"),
        }
    }

    #[tokio::test]
    async fn composite_delegates_generate_to_generator() {
        let brain = make_composite(0.0, "norm", "GENERATOR-ANSWER", "sugg", false, "m");
        let cancel = CancellationToken::new();
        let response = brain.generate(&[], &[], Locale::En, &cancel).await.unwrap();
        assert_eq!(response.answer, "GENERATOR-ANSWER");
    }

    #[tokio::test]
    async fn composite_delegates_generate_stream_to_generator() {
        use futures::StreamExt;

        let brain = make_composite(0.0, "norm", "STREAM-ANSWER", "sugg", false, "m");
        let cancel = CancellationToken::new();
        let stream = brain
            .generate_stream(&[], &[], Locale::En, &cancel)
            .await
            .unwrap();
        let chunks: Vec<_> = stream.collect::<Vec<_>>().await;
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].as_ref().unwrap().delta, "STREAM-ANSWER");
    }

    #[tokio::test]
    async fn composite_delegates_suggest_to_generator() {
        let brain = make_composite(0.0, "norm", "ans", "SUGGESTION-SENTINEL", false, "m");
        let cancel = CancellationToken::new();
        let suggestions = brain.suggest("q", "a", &cancel).await.unwrap();
        assert_eq!(suggestions, vec!["SUGGESTION-SENTINEL".to_string()]);
    }

    #[test]
    fn composite_has_web_grounding_delegates_to_generator_true() {
        let brain = make_composite(0.0, "norm", "ans", "sugg", true, "m");
        assert!(brain.has_web_grounding());
    }

    #[test]
    fn composite_has_web_grounding_delegates_to_generator_false() {
        let brain = make_composite(0.0, "norm", "ans", "sugg", false, "m");
        assert!(!brain.has_web_grounding());
    }

    #[test]
    fn composite_model_name_delegates_to_generator() {
        let brain = make_composite(0.0, "norm", "ans", "sugg", false, "gemini-test-sentinel");
        assert_eq!(brain.model_name(), "gemini-test-sentinel");
    }
}
