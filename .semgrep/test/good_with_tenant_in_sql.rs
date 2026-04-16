// Fixture: correctly-scoped query (tenant_id in the SQL string).
// The semgrep rule MUST NOT fire on this file. Not compiled.

#![allow(unused)]

use sqlx::PgPool;

pub async fn get_by_session_good_sql(
    pool: &PgPool,
    tenant_id: &str,
    session_id: &str,
) -> Vec<()> {
    let _rows = sqlx::query(
        r#"
        SELECT id, session_id, request_id
        FROM conversations
        WHERE tenant_id = $1 AND session_id = $2
        ORDER BY created_at ASC
        "#,
    )
    .bind(tenant_id)
    .bind(session_id)
    .fetch_all(pool)
    .await
    .unwrap();
    vec![]
}
