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
            if let Some((tenant, period, locale, date_str)) = parse_trending_key(&key) {
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
                            .upsert(
                                &tenant,
                                &locale,
                                &entry.query,
                                entry.score as i64,
                                &period,
                                date,
                            )
                            .await?;
                        flushed += 1;
                    }
                }

                if flushed > 0 {
                    info!(
                        key = %key,
                        tenant = %tenant,
                        flushed = flushed,
                        "Flushed trending entries"
                    );
                }
            }
        }

        Ok(())
    }
}

/// Parse a trending key like `trending:{tenant}:{period}:{locale}:{date}`
/// into `(tenant, period, locale, date)`.
///
/// Phase 3b: key shape is tenant-scoped. The 5-segment shape is the
/// only shape the worker produces from this point onward. Any legacy
/// 4-segment keys (`trending:daily:en:2026-04-14`) that remain in
/// Redis from a pre-3b deploy will simply fail to parse and be
/// ignored — they TTL out on their own (<= 7 days for weekly, <= 1
/// day for daily).
fn parse_trending_key(key: &str) -> Option<(String, TrendingPeriod, String, String)> {
    let parts: Vec<&str> = key.split(':').collect();
    if parts.len() != 5 || parts[0] != "trending" {
        return None;
    }

    let tenant = parts[1].to_string();
    let period = match parts[2] {
        "daily" => TrendingPeriod::Daily,
        "weekly" => TrendingPeriod::Weekly,
        _ => return None,
    };

    Some((tenant, period, parts[3].to_string(), parts[4].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_trending_key_daily_public_tenant() {
        let result = parse_trending_key("trending:public:daily:en:2026-04-02");
        assert!(result.is_some());
        let (tenant, period, locale, date) = result.unwrap();
        assert_eq!(tenant, "public");
        assert_eq!(period, TrendingPeriod::Daily);
        assert_eq!(locale, "en");
        assert_eq!(date, "2026-04-02");
    }

    #[test]
    fn test_parse_trending_key_weekly_custom_tenant() {
        let result = parse_trending_key("trending:acme:weekly:ja:2026-W14");
        assert!(result.is_some());
        let (tenant, period, locale, date) = result.unwrap();
        assert_eq!(tenant, "acme");
        assert_eq!(period, TrendingPeriod::Weekly);
        assert_eq!(locale, "ja");
        assert_eq!(date, "2026-W14");
    }

    #[test]
    fn test_parse_trending_key_invalid() {
        assert!(parse_trending_key("invalid:key").is_none());
        // Legacy pre-3b 4-segment shape is no longer supported — TTL out.
        assert!(parse_trending_key("trending:daily:en:2026-04-02").is_none());
        assert!(parse_trending_key("trending:public:unknown:en:2026-04-02").is_none());
        assert!(parse_trending_key("").is_none());
    }
}
