-- Revert the `users` table.
-- CITEXT extension is intentionally left installed — cheap to keep,
-- avoids breakage for any subsequent migration that expects it.
DROP TABLE IF EXISTS users;
