-- Defense in depth for the "only one active refresh batch" invariant.
--
-- The application enforces this via `swap_active_atomic` (demote prior
-- active rows in the same tx that promotes the new one) plus a session-
-- scoped advisory lock around the refresh worker. This partial unique
-- index makes the invariant a schema-level guarantee: any code path that
-- tries to leave two `status='active'` rows behind will fail loudly.

CREATE UNIQUE INDEX idx_refresh_batches_single_active
    ON refresh_batches ((status))
    WHERE status = 'active';
