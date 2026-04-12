use std::time::Duration;

use chrono::{NaiveDate, Utc};
use tokio::time;
use tracing::{error, info, instrument};

use kenjaku_core::config::TrendingConfig;
use kenjaku_core::types::trending::TrendingPeriod;
use kenjaku_infra::postgres::TrendingRepository;
use kenjaku_infra::redis::RedisClient;

/// Background worker that flushes trending queries from Redis to PostgreSQL.
pub struct TrendingFlushWorker {
    redis: RedisClient,
    repo: TrendingRepository,
    config: TrendingConfig,
}

impl TrendingFlushWorker {
    pub fn new(redis: RedisClient, repo: TrendingRepository, config: TrendingConfig) -> Self {
        Self {
            redis,
            repo,
            config,
        }
    }

    /// Run the flush worker loop.
    pub async fn run(self) {
        let interval = Duration::from_secs(self.config.flush_interval_secs);
        info!(
            interval_secs = self.config.flush_interval_secs,
            threshold = self.config.popularity_threshold,
            "Starting trending flush worker"
        );

        let mut ticker = time::interval(interval);
        loop {
            ticker.tick().await;
            if let Err(e) = self.flush_once().await {
                error!(error = %e, "Trending flush failed");
            }
        }
    }

    /// Perform a single flush cycle.
    #[instrument(skip(self))]
    async fn flush_once(&self) -> kenjaku_core::error::Result<()> {
        let pattern = "trending:*";
        let keys = self.redis.scan_keys(pattern).await?;

        for key in keys {
            if let Some((period, locale, date_str)) = parse_trending_key(&key) {
                let entries = self.redis.get_top_trending(&key, 1000).await?;

                let mut flushed = 0;
                for entry in entries {
                    if entry.score >= self.config.popularity_threshold as f64
                        && let Ok(date) =
                            NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").or_else(|_| {
                                // Parse week format: 2026-W14
                                let today = Utc::now().date_naive();
                                Ok::<NaiveDate, chrono::ParseError>(today)
                            })
                    {
                        self.repo
                            .upsert(&locale, &entry.query, entry.score as i64, &period, date)
                            .await?;
                        flushed += 1;
                    }
                }

                if flushed > 0 {
                    info!(key = %key, flushed = flushed, "Flushed trending entries");
                }
            }
        }

        Ok(())
    }
}

/// Parse a trending key like `trending:daily:en:2026-04-02` into (period, locale, date).
fn parse_trending_key(key: &str) -> Option<(TrendingPeriod, String, String)> {
    let parts: Vec<&str> = key.split(':').collect();
    if parts.len() != 4 || parts[0] != "trending" {
        return None;
    }

    let period = match parts[1] {
        "daily" => TrendingPeriod::Daily,
        "weekly" => TrendingPeriod::Weekly,
        _ => return None,
    };

    Some((period, parts[2].to_string(), parts[3].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_trending_key_daily() {
        let result = parse_trending_key("trending:daily:en:2026-04-02");
        assert!(result.is_some());
        let (period, locale, date) = result.unwrap();
        assert_eq!(period, TrendingPeriod::Daily);
        assert_eq!(locale, "en");
        assert_eq!(date, "2026-04-02");
    }

    #[test]
    fn test_parse_trending_key_weekly() {
        let result = parse_trending_key("trending:weekly:ja:2026-W14");
        assert!(result.is_some());
        let (period, locale, date) = result.unwrap();
        assert_eq!(period, TrendingPeriod::Weekly);
        assert_eq!(locale, "ja");
        assert_eq!(date, "2026-W14");
    }

    #[test]
    fn test_parse_trending_key_invalid() {
        assert!(parse_trending_key("invalid:key").is_none());
        assert!(parse_trending_key("trending:unknown:en:2026-04-02").is_none());
        assert!(parse_trending_key("").is_none());
    }
}
