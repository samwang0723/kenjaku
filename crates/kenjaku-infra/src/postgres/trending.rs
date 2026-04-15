use chrono::NaiveDate;
use sqlx::{PgPool, Row};

use kenjaku_core::error::{Error, Result};
use kenjaku_core::types::trending::{PopularQuery, TrendingPeriod};

/// Repository for popular/trending query operations.
#[derive(Clone)]
pub struct TrendingRepository {
    pool: PgPool,
}

impl TrendingRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Upsert a popular query — overwrite the stored count with the
    /// caller-supplied value (do NOT add).
    ///
    /// The flush worker passes `entry.score` (Redis's current ZSCORE)
    /// every cycle. If we added to the existing row we'd double-count
    /// every flush — a query with Redis score 5 and a 5-minute flush
    /// interval would balloon to 1,440/day in Postgres. Overwriting
    /// keeps Postgres mirrored to Redis while active keys live, and
    /// preserves the last-flushed score as the historical snapshot
    /// once the Redis key TTL-expires.
    ///
    /// Phase 3b: the INSERT now **explicitly binds** `tenant_id` rather
    /// than relying on the column's `DEFAULT 'public'`. The 3a stopgap
    /// was acceptable while no code path yet carried tenancy context;
    /// with `&TenantContext` threaded through the hot path, letting the
    /// DEFAULT silently fill the column would mask programmer error
    /// (e.g. a new repo method forgetting to bind it). Tenant-scoped
    /// INSERTs MUST supply tenant_id from here on — enforced by grep
    /// audit in QA.
    pub async fn upsert(
        &self,
        tenant_id: &str,
        locale: &str,
        query: &str,
        count: i64,
        period: &TrendingPeriod,
        period_date: NaiveDate,
    ) -> Result<()> {
        let period_str = period.to_string();

        sqlx::query(
            r#"
            INSERT INTO popular_queries (tenant_id, locale, query, search_count, period, period_date)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (tenant_id, locale, query, period, period_date)
            DO UPDATE SET search_count = EXCLUDED.search_count,
                          updated_at = NOW()
            "#,
        )
        .bind(tenant_id)
        .bind(locale)
        .bind(query)
        .bind(count)
        .bind(&period_str)
        .bind(period_date)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to upsert popular query: {e}")))?;

        Ok(())
    }

    /// Get top popular queries for a locale and period.
    ///
    /// `min_count` enforces the crowdsourcing quality floor — entries
    /// with fewer than this many searches never surface.
    ///
    /// Phase 3b: filters by `tenant_id` so cross-tenant reads are
    /// impossible.
    pub async fn get_top(
        &self,
        tenant_id: &str,
        locale: &str,
        period: &TrendingPeriod,
        limit: usize,
        min_count: i64,
    ) -> Result<Vec<PopularQuery>> {
        let period_str = period.to_string();

        let rows = sqlx::query(
            r#"
            SELECT id, locale, query, search_count, period, period_date
            FROM popular_queries
            WHERE tenant_id = $1 AND locale = $2 AND period = $3 AND search_count >= $4
            ORDER BY search_count DESC
            LIMIT $5
            "#,
        )
        .bind(tenant_id)
        .bind(locale)
        .bind(&period_str)
        .bind(min_count)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to get top queries: {e}")))?;

        rows.iter().map(row_to_popular_query).collect()
    }

    /// Get popular queries matching a prefix (for autocomplete).
    ///
    /// `min_count` enforces the crowdsourcing quality floor — entries
    /// with fewer than this many searches never surface.
    ///
    /// Phase 3b: filters by `tenant_id` so cross-tenant reads are
    /// impossible.
    pub async fn search_popular(
        &self,
        tenant_id: &str,
        locale: &str,
        prefix: &str,
        limit: usize,
        min_count: i64,
    ) -> Result<Vec<PopularQuery>> {
        let pattern = format!("{prefix}%");

        let rows = sqlx::query(
            r#"
            SELECT id, locale, query, search_count, period, period_date
            FROM popular_queries
            WHERE tenant_id = $1 AND locale = $2 AND query ILIKE $3 AND search_count >= $4
            ORDER BY search_count DESC
            LIMIT $5
            "#,
        )
        .bind(tenant_id)
        .bind(locale)
        .bind(&pattern)
        .bind(min_count)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to search popular queries: {e}")))?;

        rows.iter().map(row_to_popular_query).collect()
    }
}

fn row_to_popular_query(row: &sqlx::postgres::PgRow) -> Result<PopularQuery> {
    let period_str: String = row.get("period");
    let period: TrendingPeriod = period_str.parse()?;

    Ok(PopularQuery {
        id: row.get("id"),
        locale: row.get("locale"),
        query: row.get("query"),
        search_count: row.get("search_count"),
        period,
        period_date: row.get("period_date"),
    })
}
