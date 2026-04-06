use redis::aio::ConnectionManager;
use redis::AsyncCommands;

use kenjaku_core::config::RedisConfig;
use kenjaku_core::error::{Error, Result};
use kenjaku_core::types::trending::TrendingEntry;

/// Redis client wrapper for trending query operations.
#[derive(Clone)]
pub struct RedisClient {
    conn: ConnectionManager,
}

impl RedisClient {
    /// Create a new Redis client.
    pub async fn new(config: &RedisConfig) -> Result<Self> {
        let client = redis::Client::open(config.url.as_str())
            .map_err(|e| Error::Cache(format!("Failed to create Redis client: {e}")))?;

        let conn = ConnectionManager::new(client)
            .await
            .map_err(|e| Error::Cache(format!("Failed to connect to Redis: {e}")))?;

        Ok(Self { conn })
    }

    /// Increment a query's score in the trending sorted set.
    /// Key format: `trending:{period}:{locale}:{date}`
    pub async fn increment_trending(
        &self,
        key: &str,
        query: &str,
        ttl_secs: u64,
    ) -> Result<()> {
        let mut conn = self.conn.clone();

        // ZINCRBY key 1 member
        redis::cmd("ZINCRBY")
            .arg(key)
            .arg(1)
            .arg(query)
            .exec_async(&mut conn)
            .await
            .map_err(|e| Error::Cache(format!("ZINCRBY failed: {e}")))?;

        // Set TTL if not already set
        let ttl: i64 = redis::cmd("TTL")
            .arg(key)
            .query_async(&mut conn)
            .await
            .map_err(|e| Error::Cache(format!("TTL check failed: {e}")))?;

        if ttl < 0 {
            redis::cmd("EXPIRE")
                .arg(key)
                .arg(ttl_secs)
                .exec_async(&mut conn)
                .await
                .map_err(|e| Error::Cache(format!("EXPIRE failed: {e}")))?;
        }

        Ok(())
    }

    /// Get top entries from a trending sorted set.
    pub async fn get_top_trending(
        &self,
        key: &str,
        limit: usize,
    ) -> Result<Vec<TrendingEntry>> {
        let mut conn = self.conn.clone();

        let results: Vec<(String, f64)> = redis::cmd("ZREVRANGE")
            .arg(key)
            .arg(0)
            .arg((limit - 1) as isize)
            .arg("WITHSCORES")
            .query_async(&mut conn)
            .await
            .map_err(|e| Error::Cache(format!("ZREVRANGE failed: {e}")))?;

        Ok(results
            .into_iter()
            .map(|(query, score)| TrendingEntry { query, score })
            .collect())
    }

    /// Get all keys matching a pattern (for flush worker).
    pub async fn scan_keys(&self, pattern: &str) -> Result<Vec<String>> {
        let mut conn = self.conn.clone();

        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(pattern)
            .query_async(&mut conn)
            .await
            .map_err(|e| Error::Cache(format!("KEYS scan failed: {e}")))?;

        Ok(keys)
    }

    /// Delete a key.
    pub async fn delete_key(&self, key: &str) -> Result<()> {
        let mut conn = self.conn.clone();

        conn.del::<_, ()>(key)
            .await
            .map_err(|e| Error::Cache(format!("DEL failed: {e}")))?;

        Ok(())
    }

    /// Health check.
    pub async fn health_check(&self) -> Result<()> {
        let mut conn = self.conn.clone();

        redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map_err(|e| Error::Cache(format!("Redis health check failed: {e}")))?;

        Ok(())
    }
}
