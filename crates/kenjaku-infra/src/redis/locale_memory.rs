//! Thin Redis SET EX / GET helper for the session→locale memory.
//!
//! The service-layer `LocaleMemory` wraps this with fire-and-forget
//! semantics; this module is just the raw I/O.

use redis::aio::ConnectionManager;

use kenjaku_core::error::{Error, Result};

/// Backed by a shared `ConnectionManager` cloned from the existing
/// `RedisClient`. Cheap to clone (Arc internally).
#[derive(Clone)]
pub struct LocaleMemoryRedis {
    conn: ConnectionManager,
}

impl LocaleMemoryRedis {
    pub fn new(conn: ConnectionManager) -> Self {
        Self { conn }
    }

    /// SET key value EX ttl_seconds. Sliding TTL (every record refreshes
    /// the expiry).
    pub async fn set(&self, key: &str, value: &str, ttl_seconds: u64) -> Result<()> {
        let mut conn = self.conn.clone();
        redis::cmd("SET")
            .arg(key)
            .arg(value)
            .arg("EX")
            .arg(ttl_seconds)
            .exec_async(&mut conn)
            .await
            .map_err(|e| Error::Cache(format!("SET locale_memory failed: {e}")))?;
        Ok(())
    }

    /// GET key — returns `None` on miss.
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        let mut conn = self.conn.clone();
        let value: Option<String> = redis::cmd("GET")
            .arg(key)
            .query_async(&mut conn)
            .await
            .map_err(|e| Error::Cache(format!("GET locale_memory failed: {e}")))?;
        Ok(value)
    }
}
