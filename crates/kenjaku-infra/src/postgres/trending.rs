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

    /// Upsert a popular query (increment count or insert new).
    pub async fn upsert(
        &self,
        locale: &str,
        query: &str,
        count: i64,
        period: &TrendingPeriod,
        period_date: NaiveDate,
    ) -> Result<()> {
        let period_str = period.to_string();

        sqlx::query(
            r#"
            INSERT INTO popular_queries (locale, query, search_count, period, period_date)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (locale, query, period, period_date)
            DO UPDATE SET search_count = popular_queries.search_count + $3,
                          updated_at = NOW()
            "#,
        )
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
    pub async fn get_top(
        &self,
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
            WHERE locale = $1 AND period = $2 AND search_count >= $3
            ORDER BY search_count DESC
            LIMIT $4
            "#,
        )
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
    pub async fn search_popular(
        &self,
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
            WHERE locale = $1 AND query ILIKE $2 AND search_count >= $3
            ORDER BY search_count DESC
            LIMIT $4
            "#,
        )
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
