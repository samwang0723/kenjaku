-- Reverse migration: restore the 2-column feedback unique index.
-- Safe because all existing rows have tenant_id='public' — the
-- restored 2-column uniqueness is a weakening but still holds.

DROP INDEX IF EXISTS idx_feedback_tenant_session_request_unique;

CREATE UNIQUE INDEX idx_feedback_session_request_unique
    ON feedback(session_id, request_id);
