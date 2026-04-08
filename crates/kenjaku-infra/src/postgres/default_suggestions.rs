//! Repository for the `default_suggestions` table — the pre-materialized
//! per-locale question pool produced by `SuggestionRefreshWorker`.
//!
//! Read paths (`SuggestionService::get_top` / `autocomplete`) hit
//! `list_active_by_locale` and `prefix_search_active`. The worker hits
//! `insert_bulk`. The atomic-swap and retention SQL lives in
//! `refresh_batches.rs` since it operates on the batch state machine.

use sqlx::{PgPool, Row};
use std::str::FromStr;

use kenjaku_core::error::{Error, Result};
use kenjaku_core::types::locale::Locale;
use kenjaku_core::types::suggestion::DefaultSuggestion;

/// One pending row to insert. The worker builds a `Vec<NewDefaultSuggestion>`
/// after safety filtering and hands the whole batch to `insert_bulk`.
#[derive(Debug, Clone)]
pub struct NewDefaultSuggestion {
    pub batch_id: i64,
    pub locale: Locale,
    pub question: String,
    pub topic_cluster_id: i32,
    pub topic_label: String,
    pub weight: i32,
}

#[derive(Clone)]
pub struct DefaultSuggestionsRepository {
    pool: PgPool,
}

impl DefaultSuggestionsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Bulk insert via UNNEST. Skips rows that violate the per-batch
    /// uniqueness constraint (`ON CONFLICT DO NOTHING`) — the LLM may
    /// occasionally produce duplicates within a single locale/cluster.
    /// Returns the number of rows actually written.
    pub async fn insert_bulk(&self, rows: &[NewDefaultSuggestion]) -> Result<u64> {
        if rows.is_empty() {
            return Ok(0);
        }

        let batch_ids: Vec<i64> = rows.iter().map(|r| r.batch_id).collect();
        let locales: Vec<String> = rows.iter().map(|r| r.locale.as_str().to_string()).collect();
        let questions: Vec<String> = rows.iter().map(|r| r.question.clone()).collect();
        let questions_lower: Vec<String> = rows.iter().map(|r| r.question.to_lowercase()).collect();
        let topic_cluster_ids: Vec<i32> = rows.iter().map(|r| r.topic_cluster_id).collect();
        let topic_labels: Vec<String> = rows.iter().map(|r| r.topic_label.clone()).collect();
        let weights: Vec<i32> = rows.iter().map(|r| r.weight).collect();

        let result = sqlx::query(
            r#"
            INSERT INTO default_suggestions
                (batch_id, locale, question, question_lower, topic_cluster_id, topic_label, weight)
            SELECT * FROM UNNEST(
                $1::bigint[],
                $2::text[],
                $3::text[],
                $4::text[],
                $5::int[],
                $6::text[],
                $7::int[]
            )
            ON CONFLICT (batch_id, locale, topic_cluster_id, question_lower) DO NOTHING
            "#,
        )
        .bind(&batch_ids)
        .bind(&locales)
        .bind(&questions)
        .bind(&questions_lower)
        .bind(&topic_cluster_ids)
        .bind(&topic_labels)
        .bind(&weights)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to bulk insert default_suggestions: {e}")))?;

        Ok(result.rows_affected())
    }

    /// All currently-Active default suggestions for a locale, capped at
    /// `limit`. Read path; called on every `/top-searches` request.
    pub async fn list_active_by_locale(
        &self,
        locale: Locale,
        limit: usize,
    ) -> Result<Vec<DefaultSuggestion>> {
        let rows = sqlx::query(
            r#"
            SELECT ds.id, ds.locale, ds.question, ds.topic_cluster_id, ds.topic_label,
                   ds.batch_id, ds.generated_at, ds.weight
            FROM default_suggestions ds
            JOIN refresh_batches rb ON rb.id = ds.batch_id
            WHERE ds.locale = $1 AND rb.status = 'active'
            ORDER BY ds.weight DESC, ds.id ASC
            LIMIT $2
            "#,
        )
        .bind(locale.as_str())
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to list active default_suggestions: {e}")))?;

        rows.iter().map(row_to_default_suggestion).collect()
    }

    /// Prefix search (case-insensitive) over active rows for a locale.
    /// Caller passes the lowered prefix; the column is indexed via
    /// `text_pattern_ops` so this is a btree range scan.
    pub async fn prefix_search_active(
        &self,
        locale: Locale,
        prefix_lower: &str,
        limit: usize,
    ) -> Result<Vec<DefaultSuggestion>> {
        // Escape the LIKE wildcards in user input.
        let escaped = prefix_lower
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("{escaped}%");

        let rows = sqlx::query(
            r#"
            SELECT ds.id, ds.locale, ds.question, ds.topic_cluster_id, ds.topic_label,
                   ds.batch_id, ds.generated_at, ds.weight
            FROM default_suggestions ds
            JOIN refresh_batches rb ON rb.id = ds.batch_id
            WHERE ds.locale = $1
              AND rb.status = 'active'
              AND ds.question_lower LIKE $2 ESCAPE '\'
            ORDER BY ds.weight DESC, ds.id ASC
            LIMIT $3
            "#,
        )
        .bind(locale.as_str())
        .bind(&pattern)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            Error::Database(format!("Failed to prefix-search default_suggestions: {e}"))
        })?;

        rows.iter().map(row_to_default_suggestion).collect()
    }
}

fn row_to_default_suggestion(row: &sqlx::postgres::PgRow) -> Result<DefaultSuggestion> {
    let locale_str: String = row.get("locale");
    let locale = Locale::from_str(&locale_str).map_err(|e| {
        Error::Database(format!(
            "Unknown locale '{locale_str}' in default_suggestions row: {e}"
        ))
    })?;

    Ok(DefaultSuggestion {
        id: row.get("id"),
        locale,
        question: row.get("question"),
        topic_cluster_id: row.get("topic_cluster_id"),
        topic_label: row.get("topic_label"),
        batch_id: row.get("batch_id"),
        generated_at: row.get("generated_at"),
        weight: row.get("weight"),
    })
}
