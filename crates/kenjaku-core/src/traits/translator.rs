use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::types::search::TranslationResult;
use crate::types::usage::LlmCall;

/// Translator sub-trait extracted from the `Brain` god-trait as part of
/// the Phase 2 flexibility refactor.
///
/// A `Translator` normalizes a query into canonical English AND detects
/// the source locale in a single LLM call. Phase 2 keeps a single
/// `GeminiBrain` instance serving as the `Translator`, but the contract
/// exists so Phase 3 can route this call to a dedicated translation
/// model independently of classification or generation.
///
/// See `docs/architecture/flexibility-refactor-tech-spec.md` §3.3.3.
#[async_trait]
pub trait Translator: Send + Sync {
    /// Normalize a query into canonical English AND detect its source
    /// locale in a single LLM call.
    ///
    /// Returns the translation result paired with an optional
    /// [`LlmCall`] accounting entry.
    async fn translate(
        &self,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<(TranslationResult, Option<LlmCall>)>;
}
