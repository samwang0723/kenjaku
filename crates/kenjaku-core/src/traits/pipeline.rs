use async_trait::async_trait;

use crate::error::Result;
use crate::types::search::{SearchRequest, SearchResponse, SearchStreamOutput};
use crate::types::tenant::TenantContext;

/// A pluggable search pipeline strategy.
///
/// Implementations own the full RAG orchestration for a single search
/// request: intent classification, translation, tool fan-out, context
/// assembly, LLM generation, and component packaging.
///
/// # Today vs. intended end-state
///
/// **Today (Phase 3b):** `tctx: &TenantContext` is threaded through both
/// `search` and `search_stream`. Every request currently resolves to
/// [`TenantContext::public`] at the handler boundary — slice 3c replaces
/// that literal with an auth-extractor and starts reading per-tenant
/// context from JWT claims.
///
/// The service layer is still wired through the concrete
/// [`crate::types::search::SearchStreamOutput`]-producing pipeline because
/// the `complete_stream` finalizer is still an inherent method on
/// `SinglePassPipeline`, not part of this trait. Both `SearchService::new`
/// and `SearchOrchestrator::new` therefore accept
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
/// - **Phase 3b (LANDED)**: `tctx` is threaded. `ToolRequest` now carries
///   a private `tenant: TenantContext` field so tools can route per
///   tenant without a separate argument. Every call site currently
///   passes `TenantContext::public()` — slice 3c wires the real
///   extractor and ungates the `tenancy.enabled` flag.
/// - **Phase 3c (pending)**: `complete_stream` promotion onto this trait
///   once the auth extractor + error-code surface stabilizes. Until then
///   the constructors noted above continue to accept `Arc<SinglePassPipeline>`.
///
/// # Why `complete_stream` isn't on the trait (yet)
///
/// The current streaming contract produces a [`SearchStreamOutput`] whose
/// [`crate::types::search::StreamContext`] carries a `CancelGuard`,
/// `Instant`, and other bookkeeping that the caller threads back into
/// a completion step. That completion step currently lives as an inherent
/// method on the concrete pipeline implementation. Phase 3b added
/// `tenant: TenantContext` to `StreamContext` so the completion step has
/// the same tenancy information it needs; the promotion to the trait
/// itself is deferred to 3c/3d to keep this slice mechanical.
#[async_trait]
pub trait SearchPipeline: Send + Sync {
    /// Execute a non-streaming search.
    ///
    /// `tctx` scopes the request to a single tenant. Every repo call,
    /// cache key, Redis key, and downstream tool invocation takes its
    /// tenant identity from this argument.
    ///
    /// `device_session_id` is an optional stable per-device identity used
    /// for in-memory conversation history and locale memory. When `None`,
    /// the request's `session_id` is used as the history key.
    async fn search(
        &self,
        req: &SearchRequest,
        tctx: &TenantContext,
        device_session_id: Option<&str>,
    ) -> Result<SearchResponse>;

    /// Execute a streaming search.
    ///
    /// Returns the start-metadata block (everything knowable before the LLM
    /// begins producing tokens), a pinned token stream, and the
    /// [`crate::types::search::StreamContext`] bookkeeping needed to finish
    /// the request via the implementation's `complete_stream` inherent
    /// method. `StreamContext.tenant` carries `tctx` forward so the
    /// completion step routes its persistence to the same tenant.
    async fn search_stream(
        &self,
        req: &SearchRequest,
        tctx: &TenantContext,
        device_session_id: Option<&str>,
    ) -> Result<SearchStreamOutput>;

    /// A short, stable identifier for observability and config selection
    /// (e.g. `"single_pass"`, `"agentic"`, `"cached"`).
    fn name(&self) -> &'static str;
}
