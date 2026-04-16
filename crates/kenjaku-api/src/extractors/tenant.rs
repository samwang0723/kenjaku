//! `TenantCtx` Axum extractor (Phase 3c.2).
//!
//! A thin newtype wrapper around [`TenantContext`] that carries the
//! `FromRequestParts` impl. We need the newtype because Rust's orphan
//! rule forbids implementing the foreign `axum::FromRequestParts`
//! trait on the foreign `kenjaku_core::types::tenant::TenantContext`
//! type from the api crate.
//!
//! [`TenantCtx`] reads the request-scoped [`TenantContext`] that the
//! auth middleware inserted into request extensions. Fails **closed**
//! with 500 if the extension is missing — that's a programmer error
//! (middleware didn't run, or a handler slipped outside the global
//! layer), and silently falling back to a default context would leak
//! cross-tenant data.
//!
//! Handlers use it as a plain function argument:
//!
//! ```ignore
//! pub async fn search(
//!     TenantCtx(tctx): TenantCtx,           // <- extractor
//!     State(state): State<Arc<AppState>>,
//!     Json(dto): Json<SearchRequestDto>,
//! ) -> impl IntoResponse { /* ... */ }
//! ```
//!
//! Destructuring to `TenantCtx(tctx)` gives you the bare
//! [`TenantContext`] — the rest of the service layer takes
//! `&TenantContext`, unchanged from Phase 3b.

use std::ops::Deref;

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;

use kenjaku_core::types::tenant::TenantContext;
    use kenjaku_core::types::tenant::test_helpers::public_test_context;

/// Newtype wrapper used only as an Axum extractor. Transparently
/// derefs to the inner [`TenantContext`].
#[derive(Debug, Clone)]
pub struct TenantCtx(pub TenantContext);

impl Deref for TenantCtx {
    type Target = TenantContext;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<TenantCtx> for TenantContext {
    fn from(w: TenantCtx) -> Self {
        w.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for TenantCtx {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<TenantContext>()
            .cloned()
            .map(TenantCtx)
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::http::Request;

    fn parts_without_tctx() -> Parts {
        Request::builder().body(()).unwrap().into_parts().0
    }

    fn parts_with_tctx(tctx: TenantContext) -> Parts {
        let req = Request::builder().body(()).unwrap();
        let (mut parts, _) = req.into_parts();
        parts.extensions.insert(tctx);
        parts
    }

    #[tokio::test]
    async fn extractor_returns_tctx_when_middleware_populated() {
        let seed = public_test_context();
        let mut parts = parts_with_tctx(seed.clone());
        let TenantCtx(out) = TenantCtx::from_request_parts(&mut parts, &())
            .await
            .unwrap();
        assert_eq!(out.tenant_id.as_str(), seed.tenant_id.as_str());
        assert_eq!(out.plan_tier, seed.plan_tier);
    }

    #[tokio::test]
    async fn extractor_returns_500_when_middleware_missing() {
        // If auth middleware didn't run, the extractor must fail
        // closed — never silently fall back to `public`. Missing
        // context is a programmer error, not a user error.
        let mut parts = parts_without_tctx();
        let err = TenantCtx::from_request_parts(&mut parts, &())
            .await
            .unwrap_err();
        assert_eq!(err, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn deref_and_into_give_back_inner_tctx() {
        // Handler ergonomics check: both `*tctx` (via Deref) and
        // `let tctx: TenantContext = wrapped.into()` must work, so
        // existing `&TenantContext`-taking service APIs don't need
        // to change.
        let wrapped = TenantCtx(public_test_context());
        let tid_via_deref = wrapped.tenant_id.as_str().to_string();
        assert_eq!(tid_via_deref, "public");
        let inner: TenantContext = wrapped.into();
        assert_eq!(inner.tenant_id.as_str(), "public");
    }
}
