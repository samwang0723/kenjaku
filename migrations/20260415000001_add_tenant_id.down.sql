-- Reverse 20260415000001_add_tenant_id.up.sql.
-- Tested via up → down → up round-trip on kenjaku-postgres-1.

ALTER TABLE refresh_batches DROP COLUMN tenant_id;

DROP INDEX IF EXISTS uniq_popular_query_tenant_locale;
ALTER TABLE popular_queries DROP COLUMN tenant_id;
-- Restore the original unique constraint with its original
-- auto-generated name so any other migration that referenced it by
-- name keeps working.
ALTER TABLE popular_queries
    ADD CONSTRAINT popular_queries_locale_query_period_period_date_key
    UNIQUE (locale, query, period, period_date);

ALTER TABLE feedback DROP COLUMN tenant_id;

DROP INDEX IF EXISTS idx_conversations_tenant_created;
ALTER TABLE conversations DROP COLUMN tenant_id;

DROP TABLE tenants;
