use sqlx::{PgPool, Row};
use uuid::Uuid;

use kenjaku_core::error::{Error, Result};
use kenjaku_core::types::feedback::{CreateFeedbackRequest, Feedback, FeedbackAction, ReasonCategory};

/// Repository for feedback operations.
#[derive(Clone)]
pub struct FeedbackRepository {
    pool: PgPool,
}

impl FeedbackRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a new feedback entry.
    pub async fn create(&self, req: &CreateFeedbackRequest) -> Result<Feedback> {
        let id = Uuid::new_v4();
        let action_str = req.action.to_string();

        let row = sqlx::query(
            r#"
            INSERT INTO feedback (id, session_id, request_id, action, reason_category_id, description)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, session_id, request_id, action, reason_category_id, description, created_at
            "#,
        )
        .bind(id)
        .bind(&req.session_id)
        .bind(&req.request_id)
        .bind(&action_str)
        .bind(req.reason_category_id)
        .bind(&req.description)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to create feedback: {e}")))?;

        row_to_feedback(&row)
    }

    /// Get feedback by ID.
    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<Feedback>> {
        let row = sqlx::query(
            r#"
            SELECT id, session_id, request_id, action, reason_category_id, description, created_at
            FROM feedback WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to get feedback: {e}")))?;

        row.map(|r| row_to_feedback(&r)).transpose()
    }

    /// Get all feedback for a session.
    pub async fn get_by_session(&self, session_id: &str) -> Result<Vec<Feedback>> {
        let rows = sqlx::query(
            r#"
            SELECT id, session_id, request_id, action, reason_category_id, description, created_at
            FROM feedback WHERE session_id = $1 ORDER BY created_at DESC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to get feedback by session: {e}")))?;

        rows.iter().map(row_to_feedback).collect()
    }

    /// List all reason categories.
    pub async fn list_reason_categories(&self) -> Result<Vec<ReasonCategory>> {
        let rows = sqlx::query(
            r#"
            SELECT id, slug, label, is_active
            FROM reason_categories WHERE is_active = true
            ORDER BY id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to list reason categories: {e}")))?;

        Ok(rows
            .iter()
            .map(|row| ReasonCategory {
                id: row.get("id"),
                slug: row.get("slug"),
                label: row.get("label"),
                is_active: row.get("is_active"),
            })
            .collect())
    }
}

/// Convert a database row to a Feedback domain object.
fn row_to_feedback(row: &sqlx::postgres::PgRow) -> Result<Feedback> {
    let action_str: String = row.get("action");
    let action: FeedbackAction = action_str.parse()?;

    Ok(Feedback {
        id: row.get("id"),
        session_id: row.get("session_id"),
        request_id: row.get("request_id"),
        action,
        reason_category_id: row.get("reason_category_id"),
        description: row.get("description"),
        created_at: row.get("created_at"),
    })
}
