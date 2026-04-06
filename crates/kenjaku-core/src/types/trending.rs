use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// A popular/trending search query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PopularQuery {
    pub id: i32,
    pub locale: String,
    pub query: String,
    pub search_count: i64,
    pub period: TrendingPeriod,
    pub period_date: NaiveDate,
}

/// Time period for trending aggregation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrendingPeriod {
    Daily,
    Weekly,
}

impl std::fmt::Display for TrendingPeriod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Daily => write!(f, "daily"),
            Self::Weekly => write!(f, "weekly"),
        }
    }
}

impl std::str::FromStr for TrendingPeriod {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "daily" => Ok(Self::Daily),
            "weekly" => Ok(Self::Weekly),
            _ => Err(crate::error::Error::Validation(format!(
                "Invalid trending period: {s}"
            ))),
        }
    }
}

/// Request for top searches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopSearchesRequest {
    pub locale: String,
    #[serde(default = "default_period")]
    pub period: TrendingPeriod,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_period() -> TrendingPeriod {
    TrendingPeriod::Daily
}

fn default_limit() -> usize {
    20
}

/// A trending entry from Redis before flush to DB.
#[derive(Debug, Clone)]
pub struct TrendingEntry {
    pub query: String,
    pub score: f64,
}
