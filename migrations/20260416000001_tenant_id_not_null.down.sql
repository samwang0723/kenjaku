-- Rollback: restore DEFAULT 'public' on tenant_id columns.
ALTER TABLE conversations ALTER COLUMN tenant_id SET DEFAULT 'public';
ALTER TABLE feedback ALTER COLUMN tenant_id SET DEFAULT 'public';
ALTER TABLE popular_queries ALTER COLUMN tenant_id SET DEFAULT 'public';
ALTER TABLE refresh_batches ALTER COLUMN tenant_id SET DEFAULT 'public';
