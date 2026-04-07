-- Enforce a single feedback row per (session_id, request_id) so that
-- repeated like/dislike clicks on the same answer upsert instead of
-- creating duplicates.

-- Collapse any existing duplicates to the most recent row before
-- adding the unique constraint. Keeps the latest feedback, which is
-- the user's most recent intent.
DELETE FROM feedback f
USING feedback f2
WHERE f.session_id = f2.session_id
  AND f.request_id = f2.request_id
  AND f.created_at < f2.created_at;

CREATE UNIQUE INDEX idx_feedback_session_request_unique
    ON feedback(session_id, request_id);
