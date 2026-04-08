-- Drop default suggestions + refresh batches.
-- Order matters: default_suggestions has FK to refresh_batches.
DROP TABLE IF EXISTS default_suggestions;
DROP TABLE IF EXISTS refresh_batches;
