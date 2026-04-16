// Fixture: synthetic broken `get_by_id` query without tenant_id binding.
// The semgrep rule `tenant-scope-required` MUST fire on this file.
// Mirrors the pre-3d.2 F1 shape of FeedbackRepository::get_by_id — a
// UUID-only WHERE clause on a tenant-scoped table. Not compiled.
//
// Regression guard for 3d.2 F1 fix: if a future contributor reintroduces
// a UUID-only lookup on `feedback`, `conversations`, or any other
// tenant-scoped table, this rule catches it at CI time.

#![allow(unused)]

use sqlx::PgPool;
use uuid::Uuid;

/// Pre-fix shape — WHERE id = $1 with no tenant scoping. Rule must flag.
pub async fn get_by_id_bad(pool: &PgPool, id: Uuid) -> Option<()> {
    let _row = sqlx::query(
        r#"
        SELECT id, session_id, request_id, action
        FROM feedback WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .unwrap();
    None
}
