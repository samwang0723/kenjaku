// Fixture: synthetic broken query WITHOUT tenant_id binding.
// The semgrep rule `tenant-scope-required` MUST fire on this file.
// This file is NOT compiled — it lives under .semgrep/test/ which is
// not a Cargo workspace member.

#![allow(unused)]

use sqlx::PgPool;

/// Simulates the pre-H2 shape of ConversationRepository::get_by_session
/// (reconstructed from git show 210fdfd~1). Filters by session_id only,
/// no tenant_id predicate. Rule must flag this.
pub async fn get_by_session_bad(pool: &PgPool, session_id: &str) -> Vec<()> {
    let _rows = sqlx::query(
        r#"
        SELECT id, session_id, request_id, query, response_text
        FROM conversations
        WHERE session_id = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .unwrap();
    vec![]
}
