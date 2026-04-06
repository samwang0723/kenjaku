use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// User feedback on a search response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feedback {
    pub id: Uuid,
    pub session_id: String,
    pub request_id: String,
    pub action: FeedbackAction,
    pub reason_category_id: Option<i32>,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// The action a user took on a search response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackAction {
    Like,
    Dislike,
    Cancel,
}

impl std::fmt::Display for FeedbackAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Like => write!(f, "like"),
            Self::Dislike => write!(f, "dislike"),
            Self::Cancel => write!(f, "cancel"),
        }
    }
}

impl std::str::FromStr for FeedbackAction {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "like" => Ok(Self::Like),
            "dislike" => Ok(Self::Dislike),
            "cancel" => Ok(Self::Cancel),
            _ => Err(crate::error::Error::Validation(format!(
                "Invalid feedback action: {s}"
            ))),
        }
    }
}

/// A category for negative feedback reasons.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasonCategory {
    pub id: i32,
    pub slug: String,
    pub label: String,
    pub is_active: bool,
}

/// Request to create feedback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFeedbackRequest {
    pub session_id: String,
    pub request_id: String,
    pub action: FeedbackAction,
    pub reason_category_id: Option<i32>,
    pub description: Option<String>,
}
