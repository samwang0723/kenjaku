use async_trait::async_trait;

use crate::error::Result;
use crate::types::search::{SearchRequest, SearchResponse, SearchStreamOutput};

/// A pluggable search pipeline strategy.
///
/// Implementations own the full RAG orchestration for a single search
/// request: intent classification, translation, tool fan-out, context
/// assembly, LLM generation, and component packaging.
///
/// # Today vs. intended end-state
///
/// **Today (Phase 1):** the service layer is wired through the concrete
/// [`crate::types::search::SearchStreamOutput`]-producing pipeline
/// because the `complete_stream` finalizer is still an inherent method
/// on `SinglePassPipeline`, not part of this trait. Both
/// `SearchService::new` and `SearchOrchestrator::new` therefore accept
/// `Arc<SinglePassPipeline>` rather than `Arc<dyn SearchPipeline>`. The
/// orchestrator internally upcasts to a trait object for `search` /
/// `search_stream`, but true DI-based pipeline swapping is not yet
/// available at the composition root.
///
/// **Intended end-state:** `SearchOrchestrator` (in `kenjaku-service`)
/// will hold an `Arc<dyn SearchPipeline>` and delegate to it,
/// allowing the platform to host multiple strategies (the current
/// `SinglePassPipeline`, a future `AgenticPipeline`, a latency-optimized
/// `CachedPipeline`, …) selected by DI at startup.
///
/// # Forward-compatibility notes
///
/// - **Phase 2 (Brain decomposition)**: no signature change. Implementations
///   will compose `Classifier` + `Translator` + `Generator` sub-traits
///   internally instead of depending on the monolithic `Brain`.
/// - **Phase 3 (multi-tenancy)**: this trait is expected to gain a
///   `tctx: &TenantContext` argument once `TenantContext` lands in
///   `kenjaku-core`. At the same time, `complete_stream` will move onto
///   the trait (with a tenant-aware `StreamContext`), and the
///   constructors noted above will loosen from concrete to
///   `Arc<dyn SearchPipeline>`. That is an intentional, deferred
///   breaking change.
///
/// # Why `complete_stream` isn't on the trait (yet)
///
/// The current streaming contract produces a [`SearchStreamOutput`] whose
/// [`crate::types::search::StreamContext`] carries a `CancelGuard`,
/// `Instant`, and other bookkeeping that the caller threads back into
/// a completion step. That completion step currently lives as an inherent
/// method on the concrete pipeline implementation. Promoting it to the
/// trait requires first settling on a tenant-aware `StreamContext` shape
/// — deferred to Phase 3.
#[async_trait]
pub trait SearchPipeline: Send + Sync {
    /// Execute a non-streaming search.
    ///
    /// `device_session_id` is an optional stable per-device identity used
    /// for in-memory conversation history and locale memory. When `None`,
    /// the request's `session_id` is used as the history key.
    async fn search(
        &self,
        req: &SearchRequest,
        device_session_id: Option<&str>,
    ) -> Result<SearchResponse>;

    /// Execute a streaming search.
    ///
    /// Returns the start-metadata block (everything knowable before the LLM
    /// begins producing tokens), a pinned token stream, and the
    /// [`crate::types::search::StreamContext`] bookkeeping needed to finish
    /// the request via the implementation's `complete_stream` inherent
    /// method.
    async fn search_stream(
        &self,
        req: &SearchRequest,
        device_session_id: Option<&str>,
    ) -> Result<SearchStreamOutput>;

    /// A short, stable identifier for observability and config selection
    /// (e.g. `"single_pass"`, `"agentic"`, `"cached"`).
    fn name(&self) -> &'static str;
}
