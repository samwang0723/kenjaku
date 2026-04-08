//! Domain types for dynamic default suggestions + the refresh batch state
//! machine. See `.claude/tasks/default-suggestions-locale/tech-spec.md` §4.2.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::types::locale::Locale;

/// Whether a blended suggestion came from real crowdsourced trending data
/// or from the pre-materialized default pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionSource {
    Crowdsourced,
    Default,
}

impl SuggestionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SuggestionSource::Crowdsourced => "crowdsourced",
            SuggestionSource::Default => "default",
        }
    }
}

/// Domain-level blended item returned by `SuggestionService`.
/// The api layer converts this into `BlendedItemDto` for the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct BlendedSuggestion {
    pub query: String,
    pub source: SuggestionSource,
    /// For `Crowdsourced`: popular_queries.search_count as f64.
    /// For `Default`: default_suggestions.weight as f64.
    pub score: f64,
}

/// Lifecycle state of a refresh batch. The read path filters on `Active`;
/// the worker atomically swaps `Running` -> `Active` and the previous
/// `Active` -> `Superseded` in one transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshStatus {
    Running,
    Active,
    Superseded,
    Failed,
}

impl RefreshStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RefreshStatus::Running => "running",
            RefreshStatus::Active => "active",
            RefreshStatus::Superseded => "superseded",
            RefreshStatus::Failed => "failed",
        }
    }
}

impl FromStr for RefreshStatus {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "running" => Ok(RefreshStatus::Running),
            "active" => Ok(RefreshStatus::Active),
            "superseded" => Ok(RefreshStatus::Superseded),
            "failed" => Ok(RefreshStatus::Failed),
            other => Err(Error::Validation(format!(
                "Unknown refresh status: '{other}'. Expected running|active|superseded|failed"
            ))),
        }
    }
}

/// A single pre-materialized default suggestion row.
#[derive(Debug, Clone)]
pub struct DefaultSuggestion {
    pub id: i64,
    pub locale: Locale,
    pub question: String,
    pub topic_cluster_id: i32,
    pub topic_label: String,
    pub batch_id: i64,
    pub generated_at: DateTime<Utc>,
    pub weight: i32,
}

/// Metadata row describing one run of the refresh worker.
#[derive(Debug, Clone)]
pub struct RefreshBatch {
    pub id: i64,
    pub corpus_fingerprint: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: RefreshStatus,
    pub llm_calls: i32,
    pub questions_kept: i32,
    pub questions_rejected: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_source_serde_round_trip() {
        let crowd = SuggestionSource::Crowdsourced;
        let s = serde_json::to_string(&crowd).unwrap();
        assert_eq!(s, "\"crowdsourced\"");
        let parsed: SuggestionSource = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, crowd);

        let default = SuggestionSource::Default;
        let s = serde_json::to_string(&default).unwrap();
        assert_eq!(s, "\"default\"");
    }

    #[test]
    fn suggestion_source_as_str() {
        assert_eq!(SuggestionSource::Crowdsourced.as_str(), "crowdsourced");
        assert_eq!(SuggestionSource::Default.as_str(), "default");
    }

    #[test]
    fn refresh_status_serde_round_trip() {
        for status in [
            RefreshStatus::Running,
            RefreshStatus::Active,
            RefreshStatus::Superseded,
            RefreshStatus::Failed,
        ] {
            let s = serde_json::to_string(&status).unwrap();
            let parsed: RefreshStatus = serde_json::from_str(&s).unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn refresh_status_from_str_valid() {
        assert_eq!(
            "running".parse::<RefreshStatus>().unwrap(),
            RefreshStatus::Running
        );
        assert_eq!(
            "active".parse::<RefreshStatus>().unwrap(),
            RefreshStatus::Active
        );
        assert_eq!(
            "superseded".parse::<RefreshStatus>().unwrap(),
            RefreshStatus::Superseded
        );
        assert_eq!(
            "failed".parse::<RefreshStatus>().unwrap(),
            RefreshStatus::Failed
        );
    }

    #[test]
    fn refresh_status_from_str_invalid() {
        assert!("ACTIVE".parse::<RefreshStatus>().is_err());
        assert!("".parse::<RefreshStatus>().is_err());
        assert!("done".parse::<RefreshStatus>().is_err());
    }

    #[test]
    fn refresh_status_as_str_matches_serde() {
        for status in [
            RefreshStatus::Running,
            RefreshStatus::Active,
            RefreshStatus::Superseded,
            RefreshStatus::Failed,
        ] {
            let via_serde = serde_json::to_string(&status).unwrap();
            // Strip surrounding quotes from JSON string.
            let unquoted = via_serde.trim_matches('"').to_string();
            assert_eq!(unquoted, status.as_str());
        }
    }

    #[test]
    fn blended_suggestion_construction() {
        let item = BlendedSuggestion {
            query: "How does staking work?".to_string(),
            source: SuggestionSource::Default,
            score: 10.0,
        };
        assert_eq!(item.source, SuggestionSource::Default);
        assert!((item.score - 10.0).abs() < f64::EPSILON);
    }
}
