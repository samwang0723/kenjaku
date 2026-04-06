use sqlx::PgPool;
use tracing::instrument;
use uuid::Uuid;

use kenjaku_core::error::{Error, Result};
use kenjaku_core::types::conversation::{Conversation, CreateConversation};

/// Repository for conversation persistence.
#[derive(Clone)]
pub struct ConversationRepository {
    pool: PgPool,
}

impl ConversationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a single conversation record.
    #[instrument(skip(self, req), fields(session_id = %req.session_id, request_id = %req.request_id))]
    pub async fn create(&self, req: &CreateConversation) -> Result<Conversation> {
        let row = sqlx::query_as::<_, ConversationRow>(
            r#"
            INSERT INTO conversations (session_id, request_id, query, response_text, locale, intent, meta)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, session_id, request_id, query, response_text, locale, intent, meta, created_at
            "#,
        )
        .bind(&req.session_id)
        .bind(&req.request_id)
        .bind(&req.query)
        .bind(&req.response_text)
        .bind(req.locale.to_string())
        .bind(req.intent.to_string())
        .bind(&req.meta)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        row.try_into()
    }

    /// Batch insert conversation records (used by the flush worker).
    #[instrument(skip(self, records), fields(count = records.len()))]
    pub async fn batch_create(&self, records: &[CreateConversation]) -> Result<u64> {
        if records.is_empty() {
            return Ok(0);
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        let mut inserted: u64 = 0;
        for req in records {
            let result = sqlx::query(
                r#"
                INSERT INTO conversations (session_id, request_id, query, response_text, locale, intent, meta)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (request_id) DO NOTHING
                "#,
            )
            .bind(&req.session_id)
            .bind(&req.request_id)
            .bind(&req.query)
            .bind(&req.response_text)
            .bind(req.locale.to_string())
            .bind(req.intent.to_string())
            .bind(&req.meta)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Database(e.to_string()))?;
            inserted += result.rows_affected();
        }

        tx.commit()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        Ok(inserted)
    }

    /// Get conversations by session ID.
    #[instrument(skip(self))]
    pub async fn get_by_session(&self, session_id: &str) -> Result<Vec<Conversation>> {
        let rows = sqlx::query_as::<_, ConversationRow>(
            r#"
            SELECT id, session_id, request_id, query, response_text, locale, intent, meta, created_at
            FROM conversations
            WHERE session_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Database(e.to_string()))?;

        rows.into_iter().map(TryInto::try_into).collect()
    }
}

#[derive(sqlx::FromRow)]
struct ConversationRow {
    id: Uuid,
    session_id: String,
    request_id: String,
    query: String,
    response_text: String,
    locale: String,
    intent: String,
    meta: serde_json::Value,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<ConversationRow> for Conversation {
    type Error = Error;

    fn try_from(row: ConversationRow) -> Result<Self> {
        let locale = row
            .locale
            .parse()
            .unwrap_or(kenjaku_core::types::locale::Locale::En);
        let intent = serde_json::from_value(serde_json::Value::String(row.intent))
            .unwrap_or(kenjaku_core::types::intent::Intent::Unknown);

        Ok(Conversation {
            id: row.id,
            session_id: row.session_id,
            request_id: row.request_id,
            query: row.query,
            response_text: row.response_text,
            locale,
            intent,
            meta: row.meta,
            created_at: row.created_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenjaku_core::types::intent::Intent;
    use kenjaku_core::types::locale::Locale;

    #[test]
    fn test_conversation_row_to_domain() {
        let row = ConversationRow {
            id: Uuid::new_v4(),
            session_id: "sess-1".to_string(),
            request_id: "req-1".to_string(),
            query: "test query".to_string(),
            response_text: "test answer".to_string(),
            locale: "ja".to_string(),
            intent: "factual".to_string(),
            meta: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };

        let conv: Conversation = row.try_into().unwrap();
        assert_eq!(conv.locale, Locale::Ja);
        assert_eq!(conv.intent, Intent::Factual);
    }

    #[test]
    fn test_conversation_row_unknown_locale_defaults_to_en() {
        let row = ConversationRow {
            id: Uuid::new_v4(),
            session_id: "s".to_string(),
            request_id: "r".to_string(),
            query: "q".to_string(),
            response_text: "a".to_string(),
            locale: "xx".to_string(),
            intent: "unknown".to_string(),
            meta: serde_json::json!({}),
            created_at: chrono::Utc::now(),
        };

        let conv: Conversation = row.try_into().unwrap();
        assert_eq!(conv.locale, Locale::En);
        assert_eq!(conv.intent, Intent::Unknown);
    }
}
