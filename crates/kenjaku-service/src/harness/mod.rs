pub mod component;
pub mod context;
pub mod fanout;

use std::sync::Arc;

use kenjaku_core::error::Result;
use kenjaku_core::traits::pipeline::SearchPipeline;
use kenjaku_core::types::search::{
    LlmSource, SearchRequest, SearchResponse, SearchStreamOutput, StreamContext, StreamDoneMetadata,
};

use crate::pipelines::SinglePassPipeline;

/// Thin state holder behind `SearchService`. Delegates every operation to
/// an `Arc<dyn SearchPipeline>`.
///
/// The orchestrator keeps a typed `Arc<SinglePassPipeline>` alongside the
/// trait object for the one method not yet on the trait
/// (`complete_stream`). Phase 3 promotes `complete_stream` onto the trait
/// once `StreamContext` carries `TenantContext`; at that point the
/// `single_pass` field is deleted and only `pipeline` remains.
pub(crate) struct SearchOrchestrator {
    pipeline: Arc<dyn SearchPipeline>,
    /// Concrete handle kept so we can call `complete_stream` — not on the
    /// trait in Phase 1.
    single_pass: Arc<SinglePassPipeline>,
}

impl SearchOrchestrator {
    pub(crate) fn new(pipeline: Arc<SinglePassPipeline>) -> Self {
        let trait_obj: Arc<dyn SearchPipeline> = pipeline.clone();
        Self {
            pipeline: trait_obj,
            single_pass: pipeline,
        }
    }

    pub(crate) async fn search(
        &self,
        req: &SearchRequest,
        device_session_id: Option<&str>,
    ) -> Result<SearchResponse> {
        self.pipeline.search(req, device_session_id).await
    }

    pub(crate) async fn search_stream(
        &self,
        req: &SearchRequest,
        device_session_id: Option<&str>,
    ) -> Result<SearchStreamOutput> {
        self.pipeline.search_stream(req, device_session_id).await
    }

    pub(crate) async fn complete_stream(
        &self,
        ctx: StreamContext,
        accumulated_answer: &str,
        grounding_sources: Vec<LlmSource>,
    ) -> StreamDoneMetadata {
        self.single_pass
            .complete_stream(ctx, accumulated_answer, grounding_sources)
            .await
    }

    /// Accessor for observability: returns the underlying pipeline's name.
    #[allow(dead_code)]
    pub(crate) fn pipeline_name(&self) -> &'static str {
        self.pipeline.name()
    }
}
