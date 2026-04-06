use tracing::instrument;
use uuid::Uuid;

use kenjaku_core::error::{Error, Result};
use kenjaku_core::types::feedback::{CreateFeedbackRequest, Feedback, ReasonCategory};
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
    #[instrument(skip(self))]
    pub async fn create(&self, req: &CreateFeedbackRequest) -> Result<Feedback> {
        // Validate required fields
        if req.session_id.is_empty() {
            return Err(Error::Validation("session_id is required".to_string()));
        }
        if req.request_id.is_empty() {
            return Err(Error::Validation("request_id is required".to_string()));
        }

        self.repo.create(req).await
    }

    /// Get feedback by ID.
    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<Feedback>> {
        self.repo.get_by_id(id).await
    }

    /// Get all feedback for a session.
    pub async fn get_by_session(&self, session_id: &str) -> Result<Vec<Feedback>> {
        self.repo.get_by_session(session_id).await
    }

    /// List available reason categories.
    pub async fn list_reason_categories(&self) -> Result<Vec<ReasonCategory>> {
        self.repo.list_reason_categories().await
    }
}
