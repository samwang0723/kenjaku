-- Phase 3a: introduce multi-tenancy scaffolding.
--
-- Strictly additive, zero behavior change at runtime:
--   * `tenants` table created, pre-seeded with a single row id='public'
--   * `tenant_id TEXT NOT NULL DEFAULT 'public' REFERENCES tenants(id)`
--     column added to every table that will need tenant scoping in
--     slices 3b/3c/3d. Existing rows are backfilled to 'public' via the
--     DEFAULT; existing INSERT sites keep working unchanged.
--   * `popular_queries` unique constraint rotated from
--     `(locale, query, period, period_date)` to
--     `(tenant_id, locale, query, period, period_date)` so the same
--     query can be popular independently under different tenants.
--
-- Applied as a single migration file (sqlx runs each file in a
-- transaction by default) so the ALTER + seed + constraint rotation are
-- atomic with respect to live writers.

CREATE TABLE tenants (
    id               TEXT PRIMARY KEY,
    name             TEXT NOT NULL,
    plan_tier        TEXT NOT NULL CHECK (plan_tier IN ('free','pro','enterprise')),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    config_overrides JSONB NOT NULL DEFAULT '{}'::jsonb
);

-- The 'public' tenant is the implicit default for un-authenticated
-- requests. plan_tier='enterprise' because there are no limits on the
-- internal default tenant. MUST be inserted before any FK-bearing
-- column is added, otherwise the DEFAULT fill-in violates the FK.
INSERT INTO tenants (id, name, plan_tier) VALUES ('public', 'Public', 'enterprise');

-- conversations -------------------------------------------------------
ALTER TABLE conversations
    ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'public'
        REFERENCES tenants(id) ON DELETE RESTRICT;
CREATE INDEX idx_conversations_tenant_created
    ON conversations(tenant_id, created_at DESC);

-- feedback ------------------------------------------------------------
ALTER TABLE feedback
    ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'public'
        REFERENCES tenants(id) ON DELETE RESTRICT;

-- popular_queries -----------------------------------------------------
ALTER TABLE popular_queries
    ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'public'
        REFERENCES tenants(id) ON DELETE RESTRICT;

-- Rotate the unique constraint to be tenant-scoped. The original
-- auto-generated name was verified via `\d popular_queries` on the
-- live kenjaku-postgres-1 container before writing this migration:
--   popular_queries_locale_query_period_period_date_key
ALTER TABLE popular_queries
    DROP CONSTRAINT popular_queries_locale_query_period_period_date_key;
CREATE UNIQUE INDEX uniq_popular_query_tenant_locale
    ON popular_queries(tenant_id, locale, query, period, period_date);

-- refresh_batches -----------------------------------------------------
ALTER TABLE refresh_batches
    ADD COLUMN tenant_id TEXT NOT NULL DEFAULT 'public'
        REFERENCES tenants(id) ON DELETE RESTRICT;
