use async_trait::async_trait;

use crate::error::Result;
use crate::types::search::{SearchRequest, SearchResponse, SearchStreamOutput};

/// A pluggable search pipeline strategy.
///
/// Implementations own the full RAG orchestration for a single search
/// request: intent classification, translation, tool fan-out, context
/// assembly, LLM generation, and component packaging.
///
/// `SearchOrchestrator` (in `kenjaku-service`) holds an
/// `Arc<dyn SearchPipeline>` and delegates to it. This lets the platform
/// host multiple strategies (the current `SinglePassPipeline`, a future
/// `AgenticPipeline`, a latency-optimized `CachedPipeline`, …) selected
/// by DI at startup.
///
/// # Forward-compatibility notes
///
/// - **Phase 2 (Brain decomposition)**: no signature change. Implementations
///   will compose `Classifier` + `Translator` + `Generator` sub-traits
///   internally instead of depending on the monolithic `Brain`.
/// - **Phase 3 (multi-tenancy)**: this trait is expected to gain a
///   `tctx: &TenantContext` argument once `TenantContext` lands in
///   `kenjaku-core`. That is an intentional, deferred breaking change.
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
