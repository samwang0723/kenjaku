-- Phase 3e: tenant_id is always required. The DEFAULT 'public' was a
-- transitional crutch from 3a; all rows already have tenant_id populated.
-- Dropping the default makes it a compile-time error (via sqlx) to INSERT
-- without specifying tenant_id.

-- conversations: already NOT NULL from 3a migration, just drop default.
ALTER TABLE conversations ALTER COLUMN tenant_id DROP DEFAULT;

-- feedback: already NOT NULL from 3a migration, just drop default.
ALTER TABLE feedback ALTER COLUMN tenant_id DROP DEFAULT;

-- popular_queries: already NOT NULL from 3a migration, just drop default.
ALTER TABLE popular_queries ALTER COLUMN tenant_id DROP DEFAULT;

-- refresh_batches: already NOT NULL from 3a migration, just drop default.
ALTER TABLE refresh_batches ALTER COLUMN tenant_id DROP DEFAULT;
