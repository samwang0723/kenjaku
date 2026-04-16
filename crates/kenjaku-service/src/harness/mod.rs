pub mod component;
pub mod context;
pub mod fanout;

use std::sync::Arc;

use tracing::instrument;

use kenjaku_core::error::Result;
use kenjaku_core::traits::pipeline::SearchPipeline;
use kenjaku_core::types::search::{
    LlmSource, SearchRequest, SearchResponse, SearchStreamOutput, StreamContext, StreamDoneMetadata,
};
use kenjaku_core::types::tenant::TenantContext;
use kenjaku_core::types::usage::LlmCall;

use crate::pipelines::SinglePassPipeline;

/// Thin state holder behind `SearchService`. Delegates every operation to
/// an `Arc<dyn SearchPipeline>`.
///
/// The orchestrator keeps a typed `Arc<SinglePassPipeline>` alongside the
/// trait object for the one method not yet on the trait
/// (`complete_stream`). Phase 3b threads `&TenantContext` through every
/// method; Phase 3c/3d decides whether to promote `complete_stream` onto
/// the trait (`StreamContext` already carries `tenant`).
pub(crate) struct SearchOrchestrator {
    pipeline: Arc<dyn SearchPipeline>,
    /// Concrete handle kept so we can call `complete_stream` — not on the
    /// trait in Phase 3b.
    single_pass: Arc<SinglePassPipeline>,
}

impl SearchOrchestrator {
    /// Build an orchestrator around a single concrete pipeline.
    ///
    /// Takes `Arc<SinglePassPipeline>` rather than
    /// `Arc<dyn SearchPipeline>` in Phase 3b because
    /// [`SearchOrchestrator::complete_stream`] still calls the inherent
    /// method on the concrete pipeline (it is not yet part of the
    /// [`SearchPipeline`] trait). The struct internally upcasts to a
    /// trait object for `search` / `search_stream` delegation, so the
    /// hot path already runs through the trait.
    ///
    /// Phase 3c/3d may promote `complete_stream` onto the trait and
    /// collapse this signature to `Arc<dyn SearchPipeline>`.
    pub(crate) fn new(pipeline: Arc<SinglePassPipeline>) -> Self {
        let trait_obj: Arc<dyn SearchPipeline> = pipeline.clone();
        Self {
            pipeline: trait_obj,
            single_pass: pipeline,
        }
    }

    #[instrument(skip(self, req, tctx, device_session_id), fields(
        request_id = %req.request_id,
        tenant_id = %tctx.tenant_id.as_str(),
        plan_tier = ?tctx.plan_tier,
    ))]
    pub(crate) async fn search(
        &self,
        req: &SearchRequest,
        tctx: &TenantContext,
        device_session_id: Option<&str>,
    ) -> Result<SearchResponse> {
        self.pipeline.search(req, tctx, device_session_id).await
    }

    #[instrument(skip(self, req, tctx, device_session_id), fields(
        request_id = %req.request_id,
        tenant_id = %tctx.tenant_id.as_str(),
        plan_tier = ?tctx.plan_tier,
    ))]
    pub(crate) async fn search_stream(
        &self,
        req: &SearchRequest,
        tctx: &TenantContext,
        device_session_id: Option<&str>,
    ) -> Result<SearchStreamOutput> {
        self.pipeline
            .search_stream(req, tctx, device_session_id)
            .await
    }

    /// Finalize a streamed search. The tenant context is read from
    /// `ctx.tenant` (populated by `search_stream`) — no separate tctx
    /// argument is required.
    ///
    /// `generator_call` carries the streaming generator's token usage
    /// (harvested from the final SSE chunk's `usageMetadata`) so
    /// `StreamDoneMetadata.usage` includes it.
    pub(crate) async fn complete_stream(
        &self,
        ctx: StreamContext,
        accumulated_answer: &str,
        grounding_sources: Vec<LlmSource>,
        generator_call: Option<LlmCall>,
    ) -> StreamDoneMetadata {
        self.single_pass
            .complete_stream(ctx, accumulated_answer, grounding_sources, generator_call)
            .await
    }

    /// Accessor for observability: returns the underlying pipeline's name.
    #[allow(dead_code)]
    pub(crate) fn pipeline_name(&self) -> &'static str {
        self.pipeline.name()
    }
}
