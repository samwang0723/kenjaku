-- Dynamic default suggestions + refresh batches.
--
-- `refresh_batches` tracks each run of the daily SuggestionRefreshWorker.
-- Only one row has status='active' at a time. The worker writes a new
-- batch in 'running' state, then atomically flips the previous active
-- row to 'superseded' and the new row to 'active' in one transaction,
-- so readers always see a consistent pool.
--
-- `default_suggestions` holds the pre-materialized questions. Read path
-- filters by batch_id where status='active', enabling zero-downtime swap.
-- Cascade delete on batch_id makes retention a single delete on the parent.

CREATE TABLE refresh_batches (
    id                  BIGSERIAL PRIMARY KEY,
    corpus_fingerprint  TEXT        NOT NULL,
    status              TEXT        NOT NULL CHECK (status IN ('running','active','superseded','failed')),
    started_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at        TIMESTAMPTZ,
    llm_calls           INTEGER     NOT NULL DEFAULT 0,
    questions_kept      INTEGER     NOT NULL DEFAULT 0,
    questions_rejected  INTEGER     NOT NULL DEFAULT 0
);
CREATE INDEX idx_refresh_batches_status ON refresh_batches (status);

CREATE TABLE default_suggestions (
    id                BIGSERIAL PRIMARY KEY,
    batch_id          BIGINT      NOT NULL REFERENCES refresh_batches(id) ON DELETE CASCADE,
    locale            TEXT        NOT NULL,
    question          TEXT        NOT NULL,
    question_lower    TEXT        NOT NULL,
    topic_cluster_id  INTEGER     NOT NULL,
    topic_label       TEXT        NOT NULL,
    weight            INTEGER     NOT NULL DEFAULT 10,
    generated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (batch_id, locale, topic_cluster_id, question_lower)
);

CREATE INDEX idx_default_suggestions_batch_locale
    ON default_suggestions (batch_id, locale);
CREATE INDEX idx_default_suggestions_prefix
    ON default_suggestions (locale, question_lower text_pattern_ops);
