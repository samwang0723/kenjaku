use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::types::intent::IntentClassification;

/// Classifier sub-trait extracted from the `Brain` god-trait as part of
/// the Phase 2 flexibility refactor.
///
/// A `Classifier` is responsible for one thing: labelling a user query
/// with an `Intent`. Phase 2 keeps a single `GeminiBrain` instance serving
/// as the `Classifier`, but the contract exists so Phase 3 can swap in a
/// cheaper model (e.g. Haiku) for classification without touching the
/// pipeline or the `Brain` facade.
///
/// See `docs/architecture/flexibility-refactor-tech-spec.md` §3.3.3.
#[async_trait]
pub trait Classifier: Send + Sync {
    /// Classify the intent of a user query.
    async fn classify(
        &self,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<IntentClassification>;
}
