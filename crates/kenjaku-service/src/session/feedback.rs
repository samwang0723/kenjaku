use tracing::instrument;
use uuid::Uuid;

use kenjaku_core::error::{Error, Result};
use kenjaku_core::types::feedback::{CreateFeedbackRequest, Feedback, ReasonCategory};
use kenjaku_core::types::tenant::TenantContext;
use kenjaku_infra::postgres::FeedbackRepository;

/// Service for managing user feedback.
#[derive(Clone)]
pub struct FeedbackService {
    repo: FeedbackRepository,
}

impl FeedbackService {
    pub fn new(repo: FeedbackRepository) -> Self {
        Self { repo }
    }

    /// Create feedback with validation.
    ///
    /// Phase 3b: threads `&TenantContext` through to the repo; the
    /// INSERT explicitly binds `tenant_id`.
    #[instrument(skip(self, tctx), fields(
        tenant_id = %tctx.tenant_id.as_str(),
        plan_tier = ?tctx.plan_tier,
    ))]
    pub async fn create(
        &self,
        tctx: &TenantContext,
        req: &CreateFeedbackRequest,
    ) -> Result<Feedback> {
        // Validate required fields
        if req.session_id.is_empty() {
            return Err(Error::Validation("session_id is required".to_string()));
        }
        if req.request_id.is_empty() {
            return Err(Error::Validation("request_id is required".to_string()));
        }

        self.repo.create(tctx.tenant_id.as_str(), req).await
    }

    /// Get feedback by (tenant, id).
    ///
    /// 3d.2 F1 fix: `tctx` is required so the repo query binds
    /// `tenant_id` — a UUID known to tenant-A cannot be read by
    /// tenant-B. Currently unreached from any live handler, but the
    /// signature is tight so any future wiring is scoped by
    /// construction.
    pub async fn get_by_id(&self, tctx: &TenantContext, id: Uuid) -> Result<Option<Feedback>> {
        self.repo.get_by_id(tctx.tenant_id.as_str(), id).await
    }

    /// Get all feedback for a (tenant, session) pair.
    ///
    /// H2 (3d.1 fix): `tctx` is required so the repo query binds
    /// `tenant_id` — a session_id shared across tenants cannot
    /// cross-read. Currently unreached from any live handler.
    pub async fn get_by_session(
        &self,
        tctx: &TenantContext,
        session_id: &str,
    ) -> Result<Vec<Feedback>> {
        self.repo
            .get_by_session(tctx.tenant_id.as_str(), session_id)
            .await
    }

    /// List available reason categories.
    pub async fn list_reason_categories(&self) -> Result<Vec<ReasonCategory>> {
        self.repo.list_reason_categories().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;

    /// Regression guard for 3d.2 F1.
    ///
    /// Proves at compile time that `FeedbackService::get_by_id` requires
    /// a `&TenantContext` argument. If a future refactor drops the
    /// tenant scope (the exact pre-fix shape that leaked tenant-A rows
    /// to tenant-B callers), this test stops compiling — a much louder
    /// signal than a silent runtime regression.
    ///
    /// This is the unit-test analogue of the semgrep rule's SQL-layer
    /// check: the rule catches the SQL regression, this catches the
    /// Rust-API regression. Together they close the bug class.
    ///
    /// We can't run a live-DB test here (no integration harness in this
    /// crate — see `postgres/conversation.rs` tests for the established
    /// row-conversion-only pattern), but we CAN bind a function pointer
    /// with the required type. If the signature drifts, coercion fails.
    #[test]
    fn get_by_id_signature_requires_tenant_context() {
        type GetByIdFn = for<'a> fn(
            &'a FeedbackService,
            &'a TenantContext,
            Uuid,
        ) -> Pin<
            Box<dyn Future<Output = Result<Option<Feedback>>> + Send + 'a>,
        >;

        // If `get_by_id` ever loses `&TenantContext` (the 3d.2 F1 fix),
        // this coercion fails to type-check and the CI build is red.
        let _probe: GetByIdFn = |svc, tctx, id| Box::pin(svc.get_by_id(tctx, id));
    }

    /// Companion guard — `get_by_session` (the H2 fix from 3d.1) has the
    /// same tenant-scope contract; pin it alongside F1 so both stay
    /// locked.
    #[test]
    fn get_by_session_signature_requires_tenant_context() {
        type GetBySessionFn = for<'a> fn(
            &'a FeedbackService,
            &'a TenantContext,
            &'a str,
        ) -> Pin<
            Box<dyn Future<Output = Result<Vec<Feedback>>> + Send + 'a>,
        >;

        let _probe: GetBySessionFn =
            |svc, tctx, session_id| Box::pin(svc.get_by_session(tctx, session_id));
    }
}
