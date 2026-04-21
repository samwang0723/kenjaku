use thiserror::Error;

/// Domain-level errors for the Kenjaku system.
#[derive(Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("Vector store error: {0}")]
    VectorStore(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("Ingestion error: {0}")]
    Ingestion(String),

    #[error("Translation error: {0}")]
    Translation(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("External service error: {source}")]
    ExternalService {
        service: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("Internal error: {0}")]
    Internal(String),

    /// Tenant authentication / authorization errors carrying a stable
    /// `KNJK-XXXX` code. Introduced in Phase 3c.1; the auth middleware
    /// (3c.2) produces these; handlers turn them into the HTTP response.
    ///
    /// The `Display` impl deliberately renders the public-facing label
    /// only — no internal detail, no claim contents, no DB cursor info.
    /// Keeping the details out of `Display` prevents accidental leakage
    /// through `format!("{e}")` paths.
    #[error("{}", .0.as_public_label())]
    TenantAuth(AuthErrorCode),
}

/// Stable tenant-auth error codes.
///
/// Each variant maps to a single HTTP status and a generic user-facing
/// message — by design **no variant carries a detail string**. Specific
/// diagnostic context (JWT parse reason, missing claim name, DB error)
/// lives in `tracing` span attributes at the middleware call-site, not
/// in the error value itself. This makes it structurally impossible for
/// a handler to `format!("{e}")` and leak internals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthErrorCode {
    /// `KNJK-4010` — 401: caller failed to authenticate (no token,
    /// invalid token, expired token, wrong algorithm, wrong issuer,
    /// wrong audience, tampered signature, etc.). Generic message
    /// deliberately flattens all these cases so attackers cannot
    /// enumerate which specific check rejected them.
    Unauthorized,
    /// `KNJK-4011` — 401: login attempt with wrong email, wrong password,
    /// or a disabled user. Uniform error surface; no enumeration of
    /// which of those three conditions tripped the check.
    InvalidCredentials,
    /// `KNJK-4031` — 403: token validated but the claimed `tenant_id`
    /// is not in the tenants cache.
    TenantNotFound,
    /// `KNJK-4032` — 403: tenant exists but plan-tier / status check
    /// failed (suspended account, revoked tenant, etc.).
    TenantSuspended,
    /// `KNJK-4033` — 403: authenticated caller is not an admin for the
    /// tenant they're trying to mutate. Emitted by `require_admin`
    /// middleware on every `/api/v1/admin/*` route.
    AdminForbidden,
    /// `KNJK-4291` — 429: per-tenant rate limit exceeded (distinct from
    /// the per-IP limit, which produces a different response).
    TenantRateLimitExceeded,
    /// `KNJK-4292` — 429: per-IP login-rate limit tripped. Separate from
    /// `TenantRateLimitExceeded` (4291) because the login route runs
    /// pre-auth — no `TenantContext` exists to attribute against.
    LoginRateLimitExceeded,
    /// `KNJK-5031` — 503: pipeline variant unavailable (e.g. requested
    /// pipeline isn't registered, all replicas busy). Used by the
    /// pipeline selector in 3c.2+ flows.
    PipelineUnavailable,
}

impl AuthErrorCode {
    /// Returns the stable `KNJK-XXXX` code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized => "KNJK-4010",
            Self::InvalidCredentials => "KNJK-4011",
            Self::TenantNotFound => "KNJK-4031",
            Self::TenantSuspended => "KNJK-4032",
            Self::AdminForbidden => "KNJK-4033",
            Self::TenantRateLimitExceeded => "KNJK-4291",
            Self::LoginRateLimitExceeded => "KNJK-4292",
            Self::PipelineUnavailable => "KNJK-5031",
        }
    }

    /// Returns the HTTP status code the middleware should emit.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Unauthorized => 401,
            Self::InvalidCredentials => 401,
            Self::TenantNotFound => 403,
            Self::TenantSuspended => 403,
            Self::AdminForbidden => 403,
            Self::TenantRateLimitExceeded => 429,
            Self::LoginRateLimitExceeded => 429,
            Self::PipelineUnavailable => 503,
        }
    }

    /// Public-facing label (no internals). Used by both `Display` and
    /// [`Error::user_message`].
    pub fn as_public_label(&self) -> &'static str {
        match self {
            Self::Unauthorized => "Unauthorized tenant",
            Self::InvalidCredentials => "Invalid email or password",
            Self::TenantNotFound => "Tenant not found",
            Self::TenantSuspended => "Tenant suspended",
            Self::AdminForbidden => "Admin access required",
            Self::TenantRateLimitExceeded => "Tenant rate limit exceeded",
            Self::LoginRateLimitExceeded => "Login rate limit exceeded",
            Self::PipelineUnavailable => "Pipeline unavailable",
        }
    }
}

impl Error {
    /// Returns a safe, user-facing error message that does not leak internals.
    /// Use this in API handlers instead of `e.to_string()`.
    pub fn user_message(&self) -> &str {
        match self {
            Error::Validation(msg) => msg,
            Error::NotFound(msg) => msg,
            Error::Config(_) => "Service configuration error",
            Error::Embedding(_) => "Embedding service unavailable",
            Error::Llm(_) => "LLM service unavailable",
            Error::VectorStore(_) => "Search service unavailable",
            Error::Database(_) => "Database service unavailable",
            Error::Cache(_) => "Cache service unavailable",
            Error::Ingestion(_) => "Ingestion error",
            Error::Translation(_) => "Translation service unavailable",
            Error::ExternalService { .. } => "External service unavailable",
            Error::Internal(_) => "Internal server error",
            // Tenant auth errors: public label only. The `KNJK-XXXX`
            // code lives on the variant for log/metric attribution but
            // is deliberately NOT part of user_message() — keeping the
            // wire response free of machine-readable internal codes
            // unless the handler explicitly wants to surface them.
            Error::TenantAuth(code) => code.as_public_label(),
        }
    }
}

/// Result type alias for Kenjaku operations.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Phase 3c.1: 5 new tenant-auth error codes -----------------------

    #[test]
    fn test_auth_error_code_knjk_4010_unauthorized() {
        let code = AuthErrorCode::Unauthorized;
        assert_eq!(code.code(), "KNJK-4010");
        assert_eq!(code.http_status(), 401);
        assert_eq!(code.as_public_label(), "Unauthorized tenant");
    }

    #[test]
    fn test_auth_error_code_knjk_4031_tenant_not_found() {
        let code = AuthErrorCode::TenantNotFound;
        assert_eq!(code.code(), "KNJK-4031");
        assert_eq!(code.http_status(), 403);
        assert_eq!(code.as_public_label(), "Tenant not found");
    }

    #[test]
    fn test_auth_error_code_knjk_4032_tenant_suspended() {
        let code = AuthErrorCode::TenantSuspended;
        assert_eq!(code.code(), "KNJK-4032");
        assert_eq!(code.http_status(), 403);
        assert_eq!(code.as_public_label(), "Tenant suspended");
    }

    #[test]
    fn test_auth_error_code_knjk_4291_rate_limit() {
        let code = AuthErrorCode::TenantRateLimitExceeded;
        assert_eq!(code.code(), "KNJK-4291");
        assert_eq!(code.http_status(), 429);
        assert_eq!(code.as_public_label(), "Tenant rate limit exceeded");
    }

    #[test]
    fn test_auth_error_code_knjk_5031_pipeline_unavailable() {
        let code = AuthErrorCode::PipelineUnavailable;
        assert_eq!(code.code(), "KNJK-5031");
        assert_eq!(code.http_status(), 503);
        assert_eq!(code.as_public_label(), "Pipeline unavailable");
    }

    // ---- auth-login-rbac: three new codes (4011, 4033, 4292) -------------

    #[test]
    fn test_auth_error_code_knjk_4011_invalid_credentials() {
        let code = AuthErrorCode::InvalidCredentials;
        assert_eq!(code.code(), "KNJK-4011");
        assert_eq!(code.http_status(), 401);
        assert_eq!(code.as_public_label(), "Invalid email or password");
    }

    #[test]
    fn test_auth_error_code_knjk_4033_admin_forbidden() {
        let code = AuthErrorCode::AdminForbidden;
        assert_eq!(code.code(), "KNJK-4033");
        assert_eq!(code.http_status(), 403);
        assert_eq!(code.as_public_label(), "Admin access required");
    }

    #[test]
    fn test_auth_error_code_knjk_4292_login_rate_limit() {
        // Distinct code from 4291 (tenant rate limit) — login runs
        // pre-auth so there's no TenantContext to attribute against.
        let code = AuthErrorCode::LoginRateLimitExceeded;
        assert_eq!(code.code(), "KNJK-4292");
        assert_eq!(code.http_status(), 429);
        assert_eq!(code.as_public_label(), "Login rate limit exceeded");
    }

    // ---- user_message mapping: generic, no internal leak -----------------

    /// All AuthErrorCode variants enumerated explicitly so the
    /// compiler flags a missing entry when a new variant is added.
    fn all_auth_codes() -> [AuthErrorCode; 8] {
        [
            AuthErrorCode::Unauthorized,
            AuthErrorCode::InvalidCredentials,
            AuthErrorCode::TenantNotFound,
            AuthErrorCode::TenantSuspended,
            AuthErrorCode::AdminForbidden,
            AuthErrorCode::TenantRateLimitExceeded,
            AuthErrorCode::LoginRateLimitExceeded,
            AuthErrorCode::PipelineUnavailable,
        ]
    }

    #[test]
    fn test_tenant_auth_user_message_uses_public_label_only() {
        for code in all_auth_codes() {
            let err = Error::TenantAuth(code);
            let msg = err.user_message();
            // Public label only — no stack trace, no debug fmt, no KNJK-XXXX
            // codes bleeding into the wire payload (those live in spans).
            assert_eq!(msg, code.as_public_label());
            assert!(
                !msg.contains("KNJK-"),
                "user_message must not leak internal codes: {msg}"
            );
            assert!(
                !msg.contains("Error"),
                "user_message must not expose Rust type name: {msg}"
            );
        }
    }

    #[test]
    fn test_tenant_auth_display_matches_user_message() {
        // Defensive: if a handler accidentally uses format!("{e}"), the
        // output must still be safe. This mirrors `user_message()` on
        // purpose — both paths yield the generic label.
        for code in all_auth_codes() {
            let err = Error::TenantAuth(code);
            assert_eq!(format!("{err}"), code.as_public_label());
        }
    }

    #[test]
    fn test_auth_error_codes_are_unique() {
        // Regression guard: the `KNJK-XXXX` codes must be pairwise
        // distinct so log pipelines and runbooks can split on them
        // without collisions. Enumerates via `all_auth_codes()`.
        let codes: Vec<&'static str> = all_auth_codes().iter().map(|c| c.code()).collect();
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            codes.len(),
            "KNJK codes must be unique across variants: {codes:?}"
        );
    }

    // ---- Pre-3c.1 invariants unchanged (regression guard) ----------------

    #[test]
    fn test_validation_and_notfound_messages_passthrough() {
        // Unchanged from pre-3c.1 — Validation/NotFound carry their own
        // message verbatim. Auth codes don't; they flatten to labels.
        let v = Error::Validation("query too long".into());
        assert_eq!(v.user_message(), "query too long");
        let nf = Error::NotFound("tenant X".into());
        assert_eq!(nf.user_message(), "tenant X");
    }

    #[test]
    fn test_database_message_still_generic() {
        // Regression guard: DB errors must never surface the underlying
        // error text (connection strings, query bodies).
        let e = Error::Database("connection to postgres://user:pass@host failed".into());
        assert_eq!(e.user_message(), "Database service unavailable");
    }
}
