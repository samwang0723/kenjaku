use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use kenjaku_core::traits::collection::CollectionResolver;
use kenjaku_core::traits::retriever::Retriever;
use kenjaku_core::traits::tool::Tool;
use kenjaku_core::types::tool::{
    ToolConfig, ToolError, ToolId, ToolOutput, ToolOutputMap, ToolRequest,
};

/// Wraps the existing `HybridRetriever` (or any `dyn Retriever`) as a
/// `Tool`.
///
/// Phase 3b: holds an `Arc<dyn CollectionResolver>`. On every `invoke`,
/// the resolver maps `req.tenant().tenant_id` to the Qdrant collection
/// that owns that tenant's documents. For the `public` tenant this
/// resolves to the configured base collection name (identical behavior
/// to pre-3b); for a multi-tenant deployment it returns
/// `{base}_{tenant}`.
///
/// # Forward-compat seam — tenant cross-check (3c)
///
/// Slice 3c introduces JWT-authenticated tenancy. At that point, the
/// `resolver.resolve(&tctx.tenant_id)` call below also becomes the
/// **cross-check point**: the resolved collection name must be owned by
/// the authenticated tenant. The expected design is for the enforcement
/// to live either inside a future `DbCollectionResolver` (which reads
/// `tenants.config_overrides.collection_override` and refuses to resolve
/// collections that don't belong to the authenticated tenant) or as an
/// explicit post-resolve assertion on this same call-site. Either way,
/// 3b's structure — one resolver call, fed the tenant id, result fed
/// straight to the retriever — is the exact shape 3c needs. No refactor
/// should be required.
pub struct DocRagTool {
    retriever: Arc<dyn Retriever>,
    resolver: Arc<dyn CollectionResolver>,
    config: ToolConfig,
}

impl DocRagTool {
    pub fn new(
        retriever: Arc<dyn Retriever>,
        resolver: Arc<dyn CollectionResolver>,
        config: ToolConfig,
    ) -> Self {
        Self {
            retriever,
            resolver,
            config,
        }
    }
}

#[async_trait]
impl Tool for DocRagTool {
    fn id(&self) -> ToolId {
        ToolId("doc_rag".into())
    }

    fn config(&self) -> &ToolConfig {
        &self.config
    }

    fn should_fire(&self, req: &ToolRequest, _prior: &ToolOutputMap) -> bool {
        self.config.should_fire_for(&req.request_id)
    }

    #[instrument(skip(self, req, _prior, cancel), fields(
        request_id = %req.request_id,
        tenant_id = %req.tenant().tenant_id.as_str(),
        plan_tier = ?req.tenant().plan_tier,
    ))]
    async fn invoke(
        &self,
        req: &ToolRequest,
        _prior: &ToolOutputMap,
        cancel: &CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }

        // Phase 3b: resolver is the single source of truth for the
        // collection. Supersedes the pre-3b `req.collection_name` /
        // per-tool default fallback logic. The tctx cross-check seam
        // (slice 3c) slots in either inside this call or immediately
        // after; structure is ready.
        let collection = self
            .resolver
            .resolve(&req.tenant().tenant_id)
            .await
            .map_err(|e| ToolError::Upstream(format!("collection resolve failed: {e}")))?;

        let chunks = self
            .retriever
            .retrieve(&req.query_normalized, &collection, req.top_k)
            .await
            .map_err(|e| ToolError::Upstream(e.to_string()))?;

        Ok(ToolOutput::Chunks {
            chunks,
            provider: "doc_rag".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenjaku_core::error::Result;
    use kenjaku_core::traits::collection::PrefixCollectionResolver;
    use kenjaku_core::types::intent::Intent;
    use kenjaku_core::types::locale::Locale;
    use kenjaku_core::types::search::RetrievedChunk;
    use kenjaku_core::types::tenant::test_helpers::public_test_context;
    use kenjaku_core::types::tenant::{TenantContext, TenantId};
    use std::sync::Mutex;
    use std::sync::{
        Arc as StdArc,
        atomic::{AtomicUsize, Ordering},
    };

    /// Mock retriever that records the collection name it was queried
    /// with and returns an empty chunk list. Use `collection_seen` to
    /// assert the resolver output reached it.
    struct RecordingRetriever {
        collection_seen: Mutex<Option<String>>,
    }

    #[async_trait]
    impl Retriever for RecordingRetriever {
        async fn retrieve(
            &self,
            _query: &str,
            collection: &str,
            _top_k: usize,
        ) -> Result<Vec<RetrievedChunk>> {
            *self.collection_seen.lock().unwrap() = Some(collection.to_string());
            Ok(vec![])
        }
    }

    /// Mock resolver that records how many times it was called and
    /// what `TenantId` it saw. Returns a deterministic sentinel
    /// collection name so the retriever can prove it received it.
    struct RecordingResolver {
        call_count: AtomicUsize,
        last_tenant: Mutex<Option<String>>,
        sentinel: String,
    }

    #[async_trait]
    impl CollectionResolver for RecordingResolver {
        async fn resolve(&self, tenant: &TenantId) -> Result<String> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            *self.last_tenant.lock().unwrap() = Some(tenant.as_str().to_string());
            Ok(self.sentinel.clone())
        }
    }

    fn make_request(tctx: &TenantContext) -> ToolRequest {
        ToolRequest::new(
            "test query".into(),
            "test query".into(),
            Locale::En,
            Intent::Factual,
            // Passed but ignored in 3b — resolver overrides.
            "legacy-default".into(),
            10,
            "req-123".into(),
            "sess-456".into(),
            tctx,
        )
    }

    #[test]
    fn doc_rag_tool_should_fire_when_enabled() {
        let tool = DocRagTool::new(
            StdArc::new(RecordingRetriever {
                collection_seen: Mutex::new(None),
            }),
            StdArc::new(PrefixCollectionResolver::new("documents")),
            ToolConfig::default(),
        );
        let tctx = public_test_context();
        let req = make_request(&tctx);
        assert!(tool.should_fire(&req, &ToolOutputMap::new()));
    }

    #[test]
    fn doc_rag_tool_should_not_fire_when_disabled() {
        let tool = DocRagTool::new(
            StdArc::new(RecordingRetriever {
                collection_seen: Mutex::new(None),
            }),
            StdArc::new(PrefixCollectionResolver::new("documents")),
            ToolConfig {
                enabled: false,
                rollout_pct: None,
            },
        );
        let tctx = public_test_context();
        let req = make_request(&tctx);
        assert!(!tool.should_fire(&req, &ToolOutputMap::new()));
    }

    #[tokio::test]
    async fn doc_rag_tool_invoke_returns_chunks() {
        let tool = DocRagTool::new(
            StdArc::new(RecordingRetriever {
                collection_seen: Mutex::new(None),
            }),
            StdArc::new(PrefixCollectionResolver::new("documents")),
            ToolConfig::default(),
        );
        let tctx = public_test_context();
        let req = make_request(&tctx);
        let cancel = CancellationToken::new();
        let result = tool.invoke(&req, &ToolOutputMap::new(), &cancel).await;
        assert!(result.is_ok());
        match result.unwrap() {
            ToolOutput::Chunks { chunks, provider } => {
                assert!(chunks.is_empty());
                assert_eq!(provider, "doc_rag");
            }
            _ => panic!("expected ToolOutput::Chunks"),
        }
    }

    #[tokio::test]
    async fn doc_rag_tool_cancel_before_invoke() {
        let tool = DocRagTool::new(
            StdArc::new(RecordingRetriever {
                collection_seen: Mutex::new(None),
            }),
            StdArc::new(PrefixCollectionResolver::new("documents")),
            ToolConfig::default(),
        );
        let tctx = public_test_context();
        let req = make_request(&tctx);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = tool.invoke(&req, &ToolOutputMap::new(), &cancel).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::Cancelled => {}
            other => panic!("expected ToolError::Cancelled, got {other:?}"),
        }
    }

    // ---- Phase 3b TDD: collection resolution via resolver ----------------

    #[tokio::test]
    async fn invoke_uses_resolver_output_for_public_tenant() {
        // Given: a mock resolver returning a sentinel name, a mock
        // retriever that records the collection it was queried with,
        // and a request whose tenant is `public`.
        let resolver = StdArc::new(RecordingResolver {
            call_count: AtomicUsize::new(0),
            last_tenant: Mutex::new(None),
            sentinel: "sentinel-public-collection".into(),
        });
        let retriever = StdArc::new(RecordingRetriever {
            collection_seen: Mutex::new(None),
        });

        let tool = DocRagTool::new(retriever.clone(), resolver.clone(), ToolConfig::default());

        let tctx = public_test_context();
        let req = make_request(&tctx);

        // When: invoke runs.
        let _ = tool
            .invoke(&req, &ToolOutputMap::new(), &CancellationToken::new())
            .await
            .unwrap();

        // Then: the resolver was called exactly once with the public
        // tenant id, AND the retriever was queried with the exact
        // resolver output (not the legacy per-request collection_name,
        // not some constructor default).
        assert_eq!(resolver.call_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            resolver.last_tenant.lock().unwrap().as_deref(),
            Some("public")
        );
        assert_eq!(
            retriever.collection_seen.lock().unwrap().as_deref(),
            Some("sentinel-public-collection")
        );
    }

    #[tokio::test]
    async fn invoke_passes_tenant_id_from_request_to_resolver() {
        // Given: a request scoped to a distinct tenant (constructed
        // directly — 3b handlers still inject public, but the tool must
        // route correctly for whatever tctx it receives).
        let resolver = StdArc::new(RecordingResolver {
            call_count: AtomicUsize::new(0),
            last_tenant: Mutex::new(None),
            sentinel: "documents_acme".into(),
        });
        let retriever = StdArc::new(RecordingRetriever {
            collection_seen: Mutex::new(None),
        });
        let tool = DocRagTool::new(retriever.clone(), resolver.clone(), ToolConfig::default());
        let mut tctx = public_test_context();
        tctx.tenant_id = TenantId::new("acme").unwrap();
        let req = make_request(&tctx);

        let _ = tool
            .invoke(&req, &ToolOutputMap::new(), &CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(
            resolver.last_tenant.lock().unwrap().as_deref(),
            Some("acme")
        );
        assert_eq!(
            retriever.collection_seen.lock().unwrap().as_deref(),
            Some("documents_acme")
        );
    }
}
