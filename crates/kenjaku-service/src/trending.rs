use chrono::Utc;
use tracing::instrument;

use kenjaku_core::config::TrendingConfig;
use kenjaku_core::error::Result;
use kenjaku_core::types::trending::{PopularQuery, TrendingPeriod};
use kenjaku_infra::postgres::TrendingRepository;
use kenjaku_infra::redis::RedisClient;

/// Service for managing trending/popular search queries.
#[derive(Clone)]
pub struct TrendingService {
    redis: RedisClient,
    repo: TrendingRepository,
    config: TrendingConfig,
}

impl TrendingService {
    pub fn new(redis: RedisClient, repo: TrendingRepository, config: TrendingConfig) -> Self {
        Self {
            redis,
            repo,
            config,
        }
    }

    /// Record a search query in Redis trending sorted sets.
    #[instrument(skip(self))]
    pub async fn record_query(&self, locale: &str, query: &str) -> Result<()> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let week = Utc::now().format("%Y-W%W").to_string();

        let daily_key = format!("trending:daily:{locale}:{today}");
        let weekly_key = format!("trending:weekly:{locale}:{week}");

        // Fire and forget -- don't block the search response
        let _ = self
            .redis
            .increment_trending(&daily_key, query, self.config.daily_ttl_secs)
            .await;
        let _ = self
            .redis
            .increment_trending(&weekly_key, query, self.config.weekly_ttl_secs)
            .await;

        Ok(())
    }

    /// Get top searches from Redis (real-time) or PostgreSQL (historical).
    #[instrument(skip(self))]
    pub async fn get_top_searches(
        &self,
        locale: &str,
        period: &TrendingPeriod,
        limit: usize,
    ) -> Result<Vec<PopularQuery>> {
        // Try real-time from Redis first
        let key = match period {
            TrendingPeriod::Daily => {
                let today = Utc::now().format("%Y-%m-%d").to_string();
                format!("trending:daily:{locale}:{today}")
            }
            TrendingPeriod::Weekly => {
                let week = Utc::now().format("%Y-W%W").to_string();
                format!("trending:weekly:{locale}:{week}")
            }
        };

        let entries = self.redis.get_top_trending(&key, limit).await?;

        if !entries.is_empty() {
            let today = Utc::now().date_naive();
            return Ok(entries
                .into_iter()
                .enumerate()
                .map(|(i, entry)| PopularQuery {
                    id: i as i32,
                    locale: locale.to_string(),
                    query: entry.query,
                    search_count: entry.score as i64,
                    period: period.clone(),
                    period_date: today,
                })
                .collect());
        }

        // Fall back to PostgreSQL
        self.repo.get_top(locale, period, limit).await
    }
}
