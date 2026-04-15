//! Per-tenant collection resolution.
//!
//! Phase 3a introduces this trait as scaffolding. Slice 3b wires it into
//! `DocRagTool`/`HybridRetriever` so that each tenant reads from its own
//! Qdrant collection.
//!
//! # Naming contract
//!
//! The default [`PrefixCollectionResolver`] maps the `public` tenant to
//! the bare base name (e.g. `documents`) and every other tenant to
//! `{base}_{tenant}` (e.g. `documents_acme`). This matches
//! `tenancy.collection_name_template` in `config/base.yaml`.
//!
//! The trait is async-only so a future `DbCollectionResolver` reading
//! `tenants.config_overrides.collection_override` from Postgres slots in
//! without breaking callers.

use async_trait::async_trait;

use crate::error::Result;
use crate::types::tenant::TenantId;

/// Resolve a `TenantId` to the Qdrant collection name that owns its
/// documents.
#[async_trait]
pub trait CollectionResolver: Send + Sync {
    /// Return the collection name for `tenant`. Implementations MUST be
    /// deterministic for a given input so cache invalidation stays
    /// predictable.
    async fn resolve(&self, tenant: &TenantId) -> Result<String>;
}

/// Default resolver. `public` maps to the base name; every other tenant
/// is suffixed with `_{tenant_id}`.
///
/// `base_name` is plain config data (the Qdrant collection name from
/// `qdrant.collection_name`), not composition — `pub` is fine here.
pub struct PrefixCollectionResolver {
    pub base_name: String,
}

impl PrefixCollectionResolver {
    /// Construct a new resolver. Prefer this over struct-literal
    /// construction so future internal fields can be added without
    /// breaking callers.
    pub fn new(base_name: impl Into<String>) -> Self {
        Self {
            base_name: base_name.into(),
        }
    }
}

#[async_trait]
impl CollectionResolver for PrefixCollectionResolver {
    async fn resolve(&self, tenant: &TenantId) -> Result<String> {
        if tenant.as_str() == "public" {
            Ok(self.base_name.clone())
        } else {
            Ok(format!("{}_{}", self.base_name, tenant.as_str()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn public_tenant_returns_bare_base_name() {
        let r = PrefixCollectionResolver::new("documents");
        let out = r.resolve(&TenantId::new("public").unwrap()).await.unwrap();
        assert_eq!(out, "documents");
    }

    #[tokio::test]
    async fn non_public_tenant_gets_prefixed() {
        let r = PrefixCollectionResolver::new("documents");
        let out = r.resolve(&TenantId::new("acme").unwrap()).await.unwrap();
        assert_eq!(out, "documents_acme");
    }

    #[tokio::test]
    async fn underscores_and_dashes_preserved() {
        let r = PrefixCollectionResolver::new("documents");
        assert_eq!(
            r.resolve(&TenantId::new("x_with_underscores").unwrap())
                .await
                .unwrap(),
            "documents_x_with_underscores"
        );
        assert_eq!(
            r.resolve(&TenantId::new("x-with-dashes").unwrap())
                .await
                .unwrap(),
            "documents_x-with-dashes"
        );
    }

    #[tokio::test]
    async fn different_base_names_honored() {
        let r = PrefixCollectionResolver::new("custom_base");
        assert_eq!(
            r.resolve(&TenantId::new("public").unwrap()).await.unwrap(),
            "custom_base"
        );
        assert_eq!(
            r.resolve(&TenantId::new("t1").unwrap()).await.unwrap(),
            "custom_base_t1"
        );
    }

    #[tokio::test]
    async fn is_send_sync_for_arc_dyn_storage() {
        // Compile-time check that Arc<dyn CollectionResolver> works. 3b
        // relies on this — the resolver is held in the pipeline across
        // awaits.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PrefixCollectionResolver>();

        use std::sync::Arc;
        let r: Arc<dyn CollectionResolver> = Arc::new(PrefixCollectionResolver::new("docs"));
        let out = r.resolve(&TenantId::new("acme").unwrap()).await.unwrap();
        assert_eq!(out, "docs_acme");
    }
}
