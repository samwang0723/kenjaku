-- Tighten feedback upsert uniqueness to include tenant_id.
--
-- Phase 3b's migration (20260415000001) added tenant_id to feedback but
-- left the pre-existing UNIQUE(session_id, request_id) index untouched.
-- With tenancy disabled today all traffic is 'public' so nothing breaks,
-- but once Phase 3c enables multi-tenancy, two tenants that happen to
-- share the same (session_id, request_id) pair would collide on
-- ON CONFLICT and the DO UPDATE would overwrite the other tenant's
-- feedback row — a cross-tenant write.
--
-- Fix: replace the 2-column unique index with a tenant-scoped 3-column
-- composite. The ON CONFLICT target in
-- crates/kenjaku-infra/src/postgres/feedback.rs is updated in the same
-- commit to match. Safe on the existing 'public'-only data.
--
-- Caught by GitHub Copilot review on PR #15 (Phase 3b). Reference:
-- docs/architecture/flexibility-refactor-tech-spec.md §Phase 3.

DROP INDEX IF EXISTS idx_feedback_session_request_unique;

CREATE UNIQUE INDEX idx_feedback_tenant_session_request_unique
    ON feedback(tenant_id, session_id, request_id);
