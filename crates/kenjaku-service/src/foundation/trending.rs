use chrono::Utc;
use tracing::{debug, instrument};

use kenjaku_core::config::TrendingConfig;
use kenjaku_core::error::Result;
use kenjaku_core::types::locale::Locale;
use kenjaku_core::types::trending::{PopularQuery, TrendingPeriod};
use kenjaku_infra::postgres::TrendingRepository;
use kenjaku_infra::redis::RedisClient;

use crate::quality::{is_gibberish, normalize_for_trending};

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
    ///
    /// Applies a two-step quality policy before writing:
    /// 1. `is_gibberish` filter — drops obvious junk so it never reaches Redis.
    /// 2. `normalize_for_trending` — stores the English-normalized form for
    ///    `en` locale and a first-letter-capitalized raw query for others,
    ///    so near-duplicates collapse onto the same Redis key.
    #[instrument(skip(self, raw, normalized), fields(locale = %locale))]
    pub async fn record_query(&self, locale: Locale, raw: &str, normalized: &str) -> Result<()> {
        if is_gibberish(raw) {
            debug!(query = %raw, "Dropping gibberish query from trending");
            return Ok(());
        }

        let stored = normalize_for_trending(locale, raw, normalized);
        if stored.is_empty() {
            return Ok(());
        }

        let today = Utc::now().format("%Y-%m-%d").to_string();
        let week = Utc::now().format("%Y-W%W").to_string();
        let locale_str = locale.as_str();

        let daily_key = format!("trending:daily:{locale_str}:{today}");
        let weekly_key = format!("trending:weekly:{locale_str}:{week}");

        // Fire and forget -- don't block the search response
        let _ = self
            .redis
            .increment_trending(&daily_key, &stored, self.config.daily_ttl_secs)
            .await;
        let _ = self
            .redis
            .increment_trending(&weekly_key, &stored, self.config.weekly_ttl_secs)
            .await;

        Ok(())
    }

    /// Get top searches from Redis (real-time) or PostgreSQL (historical).
    ///
    /// Results below `crowd_sourcing_min_count` are filtered out in both
    /// paths so autocomplete and top-searches never surface one-off queries.
    #[instrument(skip(self))]
    pub async fn get_top_searches(
        &self,
        locale: &str,
        period: &TrendingPeriod,
        limit: usize,
    ) -> Result<Vec<PopularQuery>> {
        let min_count = self.config.crowd_sourcing_min_count;

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

        // Over-fetch so the min_count filter still yields `limit` entries.
        let entries = self.redis.get_top_trending(&key, limit * 4).await?;

        let filtered: Vec<_> = entries
            .into_iter()
            .filter(|e| e.score as i64 >= min_count)
            .take(limit)
            .collect();

        if !filtered.is_empty() {
            let today = Utc::now().date_naive();
            return Ok(filtered
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

        // Fall back to PostgreSQL (already filtered by min_count in the query)
        self.repo.get_top(locale, period, limit, min_count).await
    }
}
