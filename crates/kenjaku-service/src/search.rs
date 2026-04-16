use std::sync::Arc;

use tracing::warn;

use kenjaku_core::error::Result;
use kenjaku_core::types::locale::{DetectedLocale, Locale};
use kenjaku_core::types::search::{
    DetectedLocaleSource, LlmSource, SearchRequest, SearchResponse, StreamDoneMetadata,
    TranslationResult,
};
use kenjaku_core::types::tenant::TenantContext;

// Re-export streaming types for backward-compat with `kenjaku-api` callers.
// The canonical paths now live in `kenjaku_core::types::search`.
pub use kenjaku_core::types::search::{CancelGuard, SearchStreamOutput, StreamContext};

use crate::harness::SearchOrchestrator;
use crate::pipelines::SinglePassPipeline;

/// Orchestrates the full RAG search pipeline.
///
/// Public interface consumed by `kenjaku-api` handlers. Internally
/// delegates to [`SearchOrchestrator`] (a thin state holder) which in
/// turn delegates to an `Arc<dyn SearchPipeline>` — currently the default
/// [`SinglePassPipeline`].
pub struct SearchService {
    orchestrator: SearchOrchestrator,
}

impl SearchService {
    /// Construct a `SearchService` from a pre-built pipeline.
    ///
    /// DI (in `kenjaku-server/src/main.rs`) assembles a
    /// [`SinglePassPipeline`] with all its collaborators, then hands it
    /// here.
    ///
    /// **Phase 1 scope — concrete type, not a trait object.** This
    /// signature intentionally takes `Arc<SinglePassPipeline>` (rather
    /// than `Arc<dyn SearchPipeline>`) because the streaming finalizer
    /// `SearchOrchestrator::complete_stream` still calls the inherent
    /// method on the concrete pipeline. Phase 3 promotes
    /// `complete_stream` onto the [`kenjaku_core::traits::pipeline::SearchPipeline`]
    /// trait alongside the `TenantContext` rollout; at that point this
    /// constructor loosens to `Arc<dyn SearchPipeline>` and alternative
    /// variants (`AgenticPipeline`, `CachedPipeline`) become selectable
    /// purely at the composition root. See the rustdoc on
    /// `SearchPipeline` for the full roadmap.
    pub fn new(pipeline: Arc<SinglePassPipeline>) -> Self {
        Self {
            orchestrator: SearchOrchestrator::new(pipeline),
        }
    }

    /// Execute a non-streaming search.
    ///
    /// Phase 3b: threads `&TenantContext` through to the orchestrator.
    /// Handlers inject `the auth middleware's TenantContext` at the boundary (slice 3c
    /// replaces that with an auth extractor).
    pub async fn search(
        &self,
        req: &SearchRequest,
        tctx: &TenantContext,
        device_session_id: Option<&str>,
    ) -> Result<SearchResponse> {
        self.orchestrator.search(req, tctx, device_session_id).await
    }

    /// Execute a streaming search.
    ///
    /// Returns a [`SearchStreamOutput`] containing:
    /// - `start_metadata` — everything we know BEFORE the LLM stream begins
    /// - `stream` — the token delta stream
    /// - `context` — bookkeeping for `complete_stream` (carries the tctx
    ///   so the finalizer stays tenant-scoped without an extra argument)
    pub async fn search_stream(
        &self,
        req: &SearchRequest,
        tctx: &TenantContext,
        device_session_id: Option<&str>,
    ) -> Result<SearchStreamOutput> {
        self.orchestrator
            .search_stream(req, tctx, device_session_id)
            .await
    }

    /// Called by the handler after the token stream finishes. Produces the
    /// final `done` metadata and queues the conversation for async persistence.
    pub async fn complete_stream(
        &self,
        ctx: StreamContext,
        accumulated_answer: &str,
        grounding_sources: Vec<LlmSource>,
    ) -> StreamDoneMetadata {
        self.orchestrator
            .complete_stream(ctx, accumulated_answer, grounding_sources)
            .await
    }
}

/// Reconcile the translator's `Result<TranslationResult>` into the three
/// values the search pipeline needs: the English-normalized search query,
/// the resolved `Locale` to answer in, and the provenance of that locale.
///
/// Failure modes:
/// - Translator error -> `(raw_query, Locale::En, FallbackEn)` — we have
///   no normalized form to fall back to, so the raw query goes to
///   retrieval as-is.
/// - Unsupported BCP-47 tag (e.g. `pt`, `it`) -> `(tr.normalized,
///   Locale::En, FallbackEn)`. We keep the translator's English-normalized
///   form because it was successfully produced and is better for
///   retrieval than the raw non-English input; only the *answer language*
///   falls back to English.
///
/// Either way the search hot path never blocks.
pub(crate) fn resolve_translation(
    raw_query: &str,
    result: Result<TranslationResult>,
) -> (String, Locale, DetectedLocaleSource) {
    match result {
        Ok(tr) => match tr.detected_locale {
            DetectedLocale::Supported(l) => (tr.normalized, l, DetectedLocaleSource::LlmDetected),
            DetectedLocale::Unsupported { tag } => {
                warn!(
                    detected_tag = %tag,
                    "Translator detected an unsupported locale; falling back to English"
                );
                (tr.normalized, Locale::En, DetectedLocaleSource::FallbackEn)
            }
        },
        Err(e) => {
            warn!(
                error = %e,
                "Translator failed; falling back to raw query + en"
            );
            (
                raw_query.to_string(),
                Locale::En,
                DetectedLocaleSource::FallbackEn,
            )
        }
    }
}
