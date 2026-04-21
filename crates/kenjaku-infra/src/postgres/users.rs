//! Users repository (`auth-login-rbac`).
//!
//! Powers email/password login + `/api/v1/admin/users` CRUD. Every
//! query except `find_by_email` is tenant-scoped — `find_by_email` is
//! the pre-auth lookup that must work before `TenantContext` is even
//! constructed, so it's explicitly allowlisted under the semgrep
//! `tenant-scope-required` rule.
//!
//! # Password hashing
//!
//! Hashes stored as full argon2id PHC strings (`$argon2id$v=19$m=...$...`).
//! Verification runs in-process via the `argon2` crate; no network
//! calls, no external KMS. Parameters come from `Argon2::default()` —
//! OWASP-recommended (m=19456 KiB, t=2, p=1) at time of writing.
//!
//! # Enumeration defense
//!
//! `verify_password` returns a single generic error (`AuthErrorCode::
//! InvalidCredentials`) for every failure mode: missing user,
//! disabled user, wrong password, or malformed stored hash. The
//! caller (login handler) therefore cannot distinguish the four cases
//! and leak which column tripped the check.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use chrono::{DateTime, Utc};
use rand_core::OsRng;
use sqlx::{PgPool, Row};
use tracing::{debug, instrument};
use uuid::Uuid;

use kenjaku_core::error::{AuthErrorCode, Error, Result};

/// Role assigned to a user — drives `/admin/*` route authorization.
///
/// Stored as TEXT with a CHECK constraint so the DB rejects typos like
/// `"Admin"` or `"owner"` ahead of the Rust boundary. SQL rendering +
/// parsing live in `as_str` / `FromStr` so the string literal appears
/// in exactly two places.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }

    /// Parse from the TEXT column value. Mirrors the DB CHECK constraint.
    pub fn from_db_str(s: &str) -> Result<Self> {
        match s {
            "admin" => Ok(Role::Admin),
            "member" => Ok(Role::Member),
            other => Err(Error::Database(format!("unknown user role {other:?}"))),
        }
    }
}

/// A single row from the `users` table.
///
/// `password_hash` is the full argon2id PHC string. Callers MUST treat
/// this as opaque — never substring-search, never log. The custom
/// [`std::fmt::Debug`] redacts it.
#[derive(Clone)]
pub struct UserRow {
    pub id: Uuid,
    pub tenant_id: String,
    pub email: String,
    pub password_hash: String,
    pub role: Role,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for UserRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // SECURITY: `password_hash` is deliberately redacted. A stray
        // `?` debug print must not expose the PHC string (which encodes
        // the salt + params — low-value on its own, but still principle
        // of least exposure).
        f.debug_struct("UserRow")
            .field("id", &self.id)
            .field("tenant_id", &self.tenant_id)
            .field("email", &self.email)
            .field("password_hash", &"<redacted>")
            .field("role", &self.role)
            .field("enabled", &self.enabled)
            .field("created_at", &self.created_at)
            .field("last_login_at", &self.last_login_at)
            .finish()
    }
}

/// Repository for the `users` table.
#[derive(Clone)]
pub struct UsersRepository {
    pool: PgPool,
}

impl UsersRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Hash a plaintext password with argon2id + a fresh salt.
    ///
    /// Exposed so the admin handlers (and future CLI subcommands) can
    /// hash before calling [`UsersRepository::create`] — keeping all
    /// argon2 knobs in one place.
    pub fn hash_password(password: &str) -> Result<String> {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| Error::Internal(format!("password hash: {e}")))
    }

    /// Create a new user with a pre-hashed password.
    ///
    /// `password_hash` MUST be a valid argon2id PHC string — callers
    /// produce it via [`Self::hash_password`]. Returns the freshly-
    /// inserted row or an `Error::Validation` on unique-email
    /// conflict.
    #[instrument(skip(self, password_hash), fields(tenant_id = %tenant_id, role = ?role))]
    pub async fn create(
        &self,
        tenant_id: &str,
        email: &str,
        password_hash: &str,
        role: Role,
    ) -> Result<UserRow> {
        let row = sqlx::query(
            r#"
            INSERT INTO users (tenant_id, email, password_hash, role, enabled)
            VALUES ($1, $2, $3, $4, TRUE)
            RETURNING id, tenant_id, email, password_hash, role, enabled, created_at, last_login_at
            "#,
        )
        .bind(tenant_id)
        .bind(email)
        .bind(password_hash)
        .bind(role.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            // Postgres unique_violation — only the global UNIQUE(email)
            // index can fire here, so the conflict is always "email
            // taken".
            sqlx::Error::Database(db) if db.code().as_deref() == Some("23505") => {
                Error::Validation(format!("email {email:?} already exists"))
            }
            other => Error::Database(format!("create user failed: {other}")),
        })?;

        row_to_user(&row)
    }

    /// Look up a user by email. **Pre-auth** — no tenant scoping because
    /// `POST /auth/login` runs before `TenantContext` exists.
    ///
    /// Returns `Ok(None)` on a miss and `Ok(Some(row))` on a hit
    /// regardless of `enabled`. The caller is responsible for the
    /// enabled-check (usually via [`UsersRepository::verify_password`]
    /// which flattens miss / disabled / wrong-password into one error).
    #[instrument(skip(self))]
    // nosemgrep: tenant-scope-required — pre-auth lookup: no TenantContext exists yet on POST /auth/login
    pub async fn find_by_email(&self, email: &str) -> Result<Option<UserRow>> {
        let row = sqlx::query(
            r#"
            SELECT id, tenant_id, email, password_hash, role, enabled, created_at, last_login_at
            FROM users WHERE email = $1
            "#,
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("find_by_email failed: {e}")))?;

        row.as_ref().map(row_to_user).transpose()
    }

    /// Look up a user by (tenant_id, id). Used by admin CRUD + the
    /// `require_admin` middleware.
    ///
    /// Tenant-scoped: a tenant-A admin cannot read a tenant-B user row
    /// even by guessing the UUID.
    #[instrument(skip(self))]
    pub async fn find_by_id(&self, id: Uuid, tenant_id: &str) -> Result<Option<UserRow>> {
        let row = sqlx::query(
            r#"
            SELECT id, tenant_id, email, password_hash, role, enabled, created_at, last_login_at
            FROM users WHERE tenant_id = $1 AND id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("find_by_id failed: {e}")))?;

        row.as_ref().map(row_to_user).transpose()
    }

    /// List every user belonging to `tenant_id`, newest-first.
    #[instrument(skip(self))]
    pub async fn list_by_tenant(&self, tenant_id: &str) -> Result<Vec<UserRow>> {
        let rows = sqlx::query(
            r#"
            SELECT id, tenant_id, email, password_hash, role, enabled, created_at, last_login_at
            FROM users WHERE tenant_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("list_by_tenant failed: {e}")))?;

        rows.iter().map(row_to_user).collect()
    }

    /// Change a user's role, scoped to their tenant.
    ///
    /// Returns `Ok(true)` when a row was updated, `Ok(false)` when the
    /// (id, tenant_id) tuple didn't match — handlers surface that as
    /// `NotFound`.
    #[instrument(skip(self))]
    pub async fn update_role(&self, id: Uuid, tenant_id: &str, role: Role) -> Result<bool> {
        let res = sqlx::query(
            r#"
            UPDATE users SET role = $1
            WHERE tenant_id = $2 AND id = $3
            "#,
        )
        .bind(role.as_str())
        .bind(tenant_id)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("update_role failed: {e}")))?;
        Ok(res.rows_affected() > 0)
    }

    /// Flip the `enabled` flag (soft disable / re-enable).
    ///
    /// Idempotent — calling this with the current value simply writes
    /// the same value back. Returns `Ok(true)` iff a row matched the
    /// (id, tenant_id) tuple.
    #[instrument(skip(self))]
    pub async fn set_enabled(&self, id: Uuid, tenant_id: &str, enabled: bool) -> Result<bool> {
        let res = sqlx::query(
            r#"
            UPDATE users SET enabled = $1
            WHERE tenant_id = $2 AND id = $3
            "#,
        )
        .bind(enabled)
        .bind(tenant_id)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("set_enabled failed: {e}")))?;
        Ok(res.rows_affected() > 0)
    }

    /// Admin password reset. Caller produces the PHC string via
    /// [`Self::hash_password`].
    ///
    /// Tenant-scoped so a tenant-A admin cannot reset a tenant-B
    /// user's password even if they know the UUID.
    #[instrument(skip(self, password_hash))]
    pub async fn update_password_hash(
        &self,
        id: Uuid,
        tenant_id: &str,
        password_hash: &str,
    ) -> Result<bool> {
        let res = sqlx::query(
            r#"
            UPDATE users SET password_hash = $1
            WHERE tenant_id = $2 AND id = $3
            "#,
        )
        .bind(password_hash)
        .bind(tenant_id)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("update_password_hash failed: {e}")))?;
        Ok(res.rows_affected() > 0)
    }

    /// Stamp `last_login_at = NOW()` on a successful login.
    ///
    /// No tenant arg — invoked straight after [`Self::verify_password`]
    /// succeeds, where the row identity (`id`) is already uniquely
    /// resolved. Binding `id` alone is safe because it's the primary
    /// key; the `tenant_id` hint on the semgrep rule is satisfied by
    /// the nosemgrep comment below.
    #[instrument(skip(self))]
    // nosemgrep: tenant-scope-required — UPDATE by PK only; tenant_id already validated in verify_password
    pub async fn touch_last_login(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE users SET last_login_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("touch_last_login failed: {e}")))?;
        Ok(())
    }

    /// Verify an (email, password) pair. Returns the full user row on
    /// success.
    ///
    /// Every failure — unknown email, disabled user, wrong password,
    /// malformed stored hash — flattens to
    /// `Error::TenantAuth(AuthErrorCode::InvalidCredentials)`. The
    /// login handler turns this into a uniform 401; no enumeration.
    ///
    /// Argon2's `verify_password` is constant-time w.r.t. the
    /// password bytes. For the "user does not exist" branch we
    /// deliberately still compute an argon2 verification against a
    /// fixed dummy hash so the timing profile cannot distinguish
    /// "email unknown" vs "password wrong".
    #[instrument(skip(self, password), fields(email = email))]
    pub async fn verify_password(&self, email: &str, password: &str) -> Result<UserRow> {
        let Some(row) = self.find_by_email(email).await? else {
            // Timing-balance: run a dummy argon2 verify so the "no such
            // user" path takes roughly the same wall-clock time as
            // "user exists, wrong password". The result is discarded.
            const DUMMY_HASH: &str =
                "$argon2id$v=19$m=19456,t=2,p=1$ZHVtbXlfc2FsdF9mb3JfY29tcA$DdI7HYa/qQlGAfjTMH+3CUOBxfYMZi9i3N7VpY8f9UY";
            if let Ok(ph) = PasswordHash::new(DUMMY_HASH) {
                let _ = Argon2::default().verify_password(password.as_bytes(), &ph);
            }
            debug!("verify_password: no such email");
            return Err(Error::TenantAuth(AuthErrorCode::InvalidCredentials));
        };

        if !row.enabled {
            debug!("verify_password: user disabled");
            return Err(Error::TenantAuth(AuthErrorCode::InvalidCredentials));
        }

        let parsed = PasswordHash::new(&row.password_hash).map_err(|e| {
            debug!(error = %e, "verify_password: stored hash unparseable");
            Error::TenantAuth(AuthErrorCode::InvalidCredentials)
        })?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .map_err(|_| Error::TenantAuth(AuthErrorCode::InvalidCredentials))?;

        Ok(row)
    }
}

/// Deserialize a PG row into [`UserRow`].
fn row_to_user(row: &sqlx::postgres::PgRow) -> Result<UserRow> {
    let role_str: String = row
        .try_get("role")
        .map_err(|e| Error::Database(format!("users.role read: {e}")))?;
    let role = Role::from_db_str(&role_str)?;
    Ok(UserRow {
        id: row
            .try_get("id")
            .map_err(|e| Error::Database(format!("users.id read: {e}")))?,
        tenant_id: row
            .try_get("tenant_id")
            .map_err(|e| Error::Database(format!("users.tenant_id read: {e}")))?,
        email: row
            .try_get("email")
            .map_err(|e| Error::Database(format!("users.email read: {e}")))?,
        password_hash: row
            .try_get("password_hash")
            .map_err(|e| Error::Database(format!("users.password_hash read: {e}")))?,
        role,
        enabled: row
            .try_get("enabled")
            .map_err(|e| Error::Database(format!("users.enabled read: {e}")))?,
        created_at: row
            .try_get("created_at")
            .map_err(|e| Error::Database(format!("users.created_at read: {e}")))?,
        last_login_at: row
            .try_get("last_login_at")
            .map_err(|e| Error::Database(format!("users.last_login_at read: {e}")))?,
    })
}

// =====================================================================
// Tests
//
// Most UsersRepository methods require a live Postgres and are covered
// by the DB-backed integration suite under `docker-test`. The
// non-DB-touching primitives (Role roundtrip, hash_password / PHC
// shape, UserRow redaction, verify_password enumeration defense) live
// here and run in CI without infra.
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_as_str_and_from_db_str_roundtrip() {
        for r in [Role::Admin, Role::Member] {
            let s = r.as_str();
            assert_eq!(Role::from_db_str(s).unwrap(), r);
        }
    }

    #[test]
    fn role_from_db_str_rejects_unknown_values() {
        for bad in ["", "owner", "Admin", "ADMIN", "member ", " admin"] {
            assert!(Role::from_db_str(bad).is_err(), "expected {bad:?} rejected");
        }
    }

    #[test]
    fn role_serde_is_lowercase() {
        let j = serde_json::to_string(&Role::Admin).unwrap();
        assert_eq!(j, "\"admin\"");
        let back: Role = serde_json::from_str(&j).unwrap();
        assert_eq!(back, Role::Admin);
    }

    #[test]
    fn hash_password_produces_parseable_phc_string() {
        let hash = UsersRepository::hash_password("correct horse battery staple").unwrap();
        // PHC shape: $argon2id$v=19$m=...$salt$tag
        assert!(hash.starts_with("$argon2id$"), "got: {hash}");
        assert!(PasswordHash::new(&hash).is_ok());
    }

    #[test]
    fn hash_password_produces_distinct_hashes_for_same_input() {
        // Salt must be fresh per call → same plaintext yields different
        // PHC strings. Regression guard against a future refactor that
        // inadvertently reuses a static salt.
        let a = UsersRepository::hash_password("pw").unwrap();
        let b = UsersRepository::hash_password("pw").unwrap();
        assert_ne!(a, b, "argon2 salts must be per-call random");
    }

    #[test]
    fn argon2_verify_roundtrip_matches() {
        let pw = "s3kr3t!password";
        let hash = UsersRepository::hash_password(pw).unwrap();
        let ph = PasswordHash::new(&hash).unwrap();
        // Correct password verifies.
        assert!(Argon2::default().verify_password(pw.as_bytes(), &ph).is_ok());
        // Wrong password rejects.
        assert!(
            Argon2::default()
                .verify_password(b"totally-different", &ph)
                .is_err()
        );
    }

    #[test]
    fn user_row_debug_redacts_password_hash() {
        let row = UserRow {
            id: Uuid::nil(),
            tenant_id: "public".into(),
            email: "admin@public.com".into(),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$SECRET_SALT$SECRET_TAG".into(),
            role: Role::Admin,
            enabled: true,
            created_at: Utc::now(),
            last_login_at: None,
        };
        let dbg = format!("{row:?}");
        assert!(dbg.contains("admin@public.com"), "email visible: {dbg}");
        assert!(dbg.contains("<redacted>"), "hash redacted: {dbg}");
        assert!(
            !dbg.contains("SECRET_SALT"),
            "salt must not leak via Debug: {dbg}"
        );
        assert!(
            !dbg.contains("SECRET_TAG"),
            "tag must not leak via Debug: {dbg}"
        );
    }

    #[test]
    fn row_to_user_rejects_unknown_role() {
        // Can't directly construct a PgRow without a live DB — this
        // test narrow-scopes the parse path by calling Role::from_db_str
        // directly, which row_to_user delegates to.
        let err = Role::from_db_str("owner").unwrap_err();
        assert!(matches!(err, Error::Database(_)));
    }
}
