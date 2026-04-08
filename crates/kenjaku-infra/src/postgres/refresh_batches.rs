//! Repository for `refresh_batches` — the state machine row that drives
//! the atomic active-batch swap. See spec §4.4 and §5.1.

use sqlx::{PgPool, Row};
use std::str::FromStr;

use kenjaku_core::error::{Error, Result};
use kenjaku_core::types::suggestion::{RefreshBatch, RefreshStatus};

#[derive(Clone)]
pub struct RefreshBatchesRepository {
    pool: PgPool,
}

impl RefreshBatchesRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new batch row in `running` status. Returns its id.
    pub async fn start_batch(&self, corpus_fingerprint: &str) -> Result<i64> {
        let row = sqlx::query(
            r#"
            INSERT INTO refresh_batches (corpus_fingerprint, status)
            VALUES ($1, 'running')
            RETURNING id
            "#,
        )
        .bind(corpus_fingerprint)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to start refresh batch: {e}")))?;
        Ok(row.get::<i64, _>("id"))
    }

    /// Get the most recent `Active` batch (or `None` if there is none).
    pub async fn latest_active(&self) -> Result<Option<RefreshBatch>> {
        let row = sqlx::query(
            r#"
            SELECT id, corpus_fingerprint, started_at, completed_at, status,
                   llm_calls, questions_kept, questions_rejected
            FROM refresh_batches
            WHERE status = 'active'
            ORDER BY started_at DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to load latest active batch: {e}")))?;

        row.as_ref().map(row_to_refresh_batch).transpose()
    }

    /// Atomic-swap transaction: in one tx flip every existing `active`
    /// row to `superseded`, then promote the new batch from `running` to
    /// `active`. Read path filters on `status='active'`, so the swap is
    /// instant and zero-downtime.
    pub async fn swap_active_atomic(
        &self,
        new_batch_id: i64,
        llm_calls: i32,
        questions_kept: i32,
        questions_rejected: i32,
    ) -> Result<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::Database(format!("Failed to start swap tx: {e}")))?;

        sqlx::query(
            r#"
            UPDATE refresh_batches
               SET status = 'superseded'
             WHERE status = 'active' AND id <> $1
            "#,
        )
        .bind(new_batch_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::Database(format!("Failed to supersede prior active batches: {e}")))?;

        sqlx::query(
            r#"
            UPDATE refresh_batches
               SET status = 'active',
                   completed_at = NOW(),
                   llm_calls = $2,
                   questions_kept = $3,
                   questions_rejected = $4
             WHERE id = $1
            "#,
        )
        .bind(new_batch_id)
        .bind(llm_calls)
        .bind(questions_kept)
        .bind(questions_rejected)
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::Database(format!("Failed to promote new batch to active: {e}")))?;

        tx.commit()
            .await
            .map_err(|e| Error::Database(format!("Failed to commit swap tx: {e}")))?;
        Ok(())
    }

    /// Mark a batch failed and stamp completed_at. Leaves any prior
    /// `active` batch untouched, so reads keep serving the old data.
    pub async fn mark_failed(&self, batch_id: i64) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE refresh_batches
               SET status = 'failed', completed_at = NOW()
             WHERE id = $1
            "#,
        )
        .bind(batch_id)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to mark batch failed: {e}")))?;
        Ok(())
    }

    /// Retention: keep only the most recent `n` batches by `started_at`,
    /// cascade-deleting the rest. `n` is small (default 3 per spec).
    /// Cascade is enforced by the FK on `default_suggestions.batch_id`.
    pub async fn retain_last_n(&self, n: usize) -> Result<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM refresh_batches
             WHERE id IN (
                 SELECT id FROM refresh_batches
                 ORDER BY started_at DESC
                 OFFSET $1
             )
            "#,
        )
        .bind(n as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("Failed to apply retention: {e}")))?;
        Ok(result.rows_affected())
    }
}

fn row_to_refresh_batch(row: &sqlx::postgres::PgRow) -> Result<RefreshBatch> {
    let status_str: String = row.get("status");
    let status = RefreshStatus::from_str(&status_str)?;

    Ok(RefreshBatch {
        id: row.get("id"),
        corpus_fingerprint: row.get("corpus_fingerprint"),
        started_at: row.get("started_at"),
        completed_at: row.get("completed_at"),
        status,
        llm_calls: row.get("llm_calls"),
        questions_kept: row.get("questions_kept"),
        questions_rejected: row.get("questions_rejected"),
    })
}
