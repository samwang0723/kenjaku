-- Email/password login + RBAC scaffolding (auth-login-rbac).
--
-- Introduces the `users` table that `POST /api/v1/auth/login` authenticates
-- against, and the `/api/v1/admin/users` endpoints manage.
--
-- Design choices:
--   * CITEXT email column so `admin@public.com` and `Admin@Public.COM` collide
--     on the global UNIQUE index. Less footgun-prone than `LOWER(email)` +
--     functional index — other code paths that read the column don't need
--     to remember to call `LOWER` first.
--   * `password_hash TEXT` stores the full argon2id PHC string (includes
--     params + salt); verification happens in-process via the `argon2`
--     crate. No plaintext ever touches the DB.
--   * `role` is a plain TEXT CHECK constraint — only two roles in v1 so
--     an enum type would be overkill. Adding roles later is a simple CHECK
--     constraint bump.
--   * `enabled` flag = soft-disable instead of hard-delete. Keeps the audit
--     trail + preserves FK fan-out when later schemas reference `users.id`.
--   * `tenant_id` FK points at `tenants(id)`. `ON DELETE CASCADE` so
--     tearing down a tenant automatically removes its users. A tenant
--     cannot exist without at least the seed admin for the `public`
--     tenant below.
--
-- Seeds a default `admin@public.com` / `admin` user with an argon2id hash
-- pre-computed locally (one-off, using the `argon2` crate with default
-- OWASP params). Idempotent via `ON CONFLICT (email) DO NOTHING`.
-- Production deployments MUST rotate this password via the admin UI or
-- `POST /api/v1/admin/users/:id/reset-password` endpoint before exposing
-- the system.

CREATE EXTENSION IF NOT EXISTS citext;

CREATE TABLE users (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id      TEXT NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    email          CITEXT NOT NULL,
    password_hash  TEXT NOT NULL,
    role           TEXT NOT NULL CHECK (role IN ('admin','member')),
    enabled        BOOLEAN NOT NULL DEFAULT TRUE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_login_at  TIMESTAMPTZ,
    UNIQUE (email)
);

CREATE INDEX idx_users_tenant ON users(tenant_id);

-- Seed default admin user for the pre-existing `public` tenant.
-- Hash generated via: argon2::PasswordHasher on input bytes b"admin"
-- with Argon2::default() params (argon2id, m=19456 KiB, t=2, p=1).
-- Operators should rotate this credential immediately on any shared deploy.
INSERT INTO users (tenant_id, email, password_hash, role, enabled)
VALUES (
    'public',
    'admin@public.com',
    '$argon2id$v=19$m=19456,t=2,p=1$kV5cbB/pe1yU0qbAQ+DrJg$LYugS7g4BpDxojre7o8MtG9ul+oOxsdQgVkH17x9nEU',
    'admin',
    TRUE
)
ON CONFLICT (email) DO NOTHING;
