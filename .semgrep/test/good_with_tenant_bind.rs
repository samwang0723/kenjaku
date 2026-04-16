// Fixture: query where tenant_id appears in the SQL as a table-qualified
// column reference (`c.tenant_id`). The rule MUST NOT fire for this
// qualified SQL usage in the matched builder chain.
// Not compiled.

#![allow(unused)]

use sqlx::PgPool;

pub async fn get_by_session_good_bind(
    pool: &PgPool,
    tctx_tenant_id: &str,
    session_id: &str,
) -> Vec<()> {
    let _rows = sqlx::query(
        r#"
        SELECT c.id, c.session_id
        FROM conversations c
        WHERE c.tenant_id = $1 AND c.session_id = $2
        "#,
    )
    .bind(tctx_tenant_id)
    .bind(session_id)
    .fetch_all(pool)
    .await
    .unwrap();
    vec![]
}
