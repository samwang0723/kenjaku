//! Merged preamble result type for the two-call pipeline experiment.
//!
//! The classic pipeline issues `classify_intent` and `translate` as two
//! independent LLM calls running in parallel via `tokio::join!`. The
//! `two_call` pipeline mode replaces those with a single Gemini call
//! that returns intent + translation + locale-detection in one
//! structured-output JSON response.
//!
//! [`QueryPreprocessing`] is the unified return shape regardless of
//! which path produced it. Pipeline code consumes this struct directly
//! and never branches on the call mode.

use serde::{Deserialize, Serialize};

use crate::types::intent::IntentClassification;
use crate::types::search::TranslationResult;

/// Combined output of the preamble step (intent + translation).
///
/// Produced either by:
/// - Two parallel LLM calls (`pipeline.mode = single_pass`, today's default), or
/// - One merged LLM call with structured JSON output (`pipeline.mode = two_call`).
#[derive(Debug, Clone)]
pub struct QueryPreprocessing {
    pub intent: IntentClassification,
    pub translation: TranslationResult,
}

/// Wire-format payload Gemini returns for the merged preamble call.
/// Mirrors the `responseSchema` declared in `GeminiProvider::preprocess_query`.
///
/// Kept as a stable serde struct so the test suite can assert on it
/// without touching provider-private deserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreprocessWire {
    /// One of: factual, navigational, how_to, comparison, troubleshooting,
    /// exploratory, conversational, unknown.
    pub intent: String,
    /// BCP-47 source language tag (e.g. "en", "zh", "zh-TW", "ja").
    pub detected_locale: String,
    /// Query rewritten in canonical English.
    pub normalized_query: String,
}
