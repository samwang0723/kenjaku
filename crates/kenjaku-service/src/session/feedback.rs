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

    /// Get feedback by ID.
    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<Feedback>> {
        self.repo.get_by_id(id).await
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
