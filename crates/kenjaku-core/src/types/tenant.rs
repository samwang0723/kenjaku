//! Tenant identity and context.
//!
//! Phase 3a introduces these types as scaffolding. Nothing reads them yet —
//! slice 3b threads `&TenantContext` through `SearchPipeline::search`, slice
//! 3c wires the JWT/header extractor, slice 3d adds isolation tests and
//! metrics.
//!
//! # Forward-compat contract
//!
//! - `TenantContext: Clone` so it threads through async code without
//!   lifetimes.
//! - `principal_id: Option<PrincipalId>` exists in 3a so 3c's JWT extractor
//!   can populate it without a schema change.
//! - `plan_tier: PlanTier` exists in 3a so 3c's rate-limit extractor + 3d
//!   metrics can branch on it without touching this struct again.
//! - Fields are `pub` by design — these are domain types, not composition
//!   wrappers. Matches `Locale`/`DefaultSuggestion` precedent.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Maximum byte length of a `TenantId` or `PrincipalId`.
///
/// Matches the intuition of a `VARCHAR(128)` column even though the actual
/// Postgres column is `TEXT` (no server-side length cap). Keeps URL paths,
/// log lines, and cache keys sane.
pub const MAX_ID_LEN: usize = 128;

/// Stable identifier for a tenant (organization / workspace).
///
/// Validated: non-empty, ≤ 128 bytes, charset `[A-Za-z0-9_-]`. The charset
/// rule rejects path-traversal characters (`.`, `/`, `\`), SQL/shell meta
/// (`'`, `"`, `;`, `` ` ``), whitespace, null bytes, and non-ASCII — every
/// tenant id is safe to splice into a Qdrant collection name, a Redis key,
/// or a log line without further escaping.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TenantId(String);

impl TenantId {
    /// Construct a validated `TenantId`.
    ///
    /// Returns `Error::Validation` on empty input, input longer than
    /// [`MAX_ID_LEN`] bytes, or input containing characters outside
    /// `[A-Za-z0-9_-]`.
    pub fn new(raw: impl Into<String>) -> Result<Self> {
        let raw = raw.into();
        validate_id(&raw, "tenant_id")?;
        Ok(Self(raw))
    }

    /// Borrow the underlying string. Guaranteed non-empty, ASCII-only,
    /// charset-restricted per `new`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for TenantId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::new(s)
    }
}

impl TryFrom<String> for TenantId {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<TenantId> for String {
    fn from(id: TenantId) -> String {
        id.0
    }
}

/// Stable identifier for a principal (user / service account / API key).
///
/// Same validation rules as [`TenantId`] — 3c populates this from JWT
/// claims; keeping the shape identical simplifies the auth extractor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PrincipalId(String);

impl PrincipalId {
    /// Construct a validated `PrincipalId`. See [`TenantId::new`].
    pub fn new(raw: impl Into<String>) -> Result<Self> {
        let raw = raw.into();
        validate_id(&raw, "principal_id")?;
        Ok(Self(raw))
    }

    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for PrincipalId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::new(s)
    }
}

impl TryFrom<String> for PrincipalId {
    type Error = Error;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<PrincipalId> for String {
    fn from(id: PrincipalId) -> String {
        id.0
    }
}

/// Plan tier of a tenant. Drives rate-limit bucket sizes (3c) and
/// feature flag gates. `Enterprise` is the implicit tier for the `public`
/// internal tenant — no limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanTier {
    Free,
    Pro,
    Enterprise,
}

/// Request-scoped tenancy context. Constructed per-request by the (3c)
/// auth extractor and threaded (3b) through `SearchPipeline::search` +
/// every `Tool::invoke`. In 3a this is inert — nothing reads it.
#[derive(Debug, Clone)]
pub struct TenantContext {
    pub tenant_id: TenantId,
    pub principal_id: Option<PrincipalId>,
    pub plan_tier: PlanTier,
}

impl TenantContext {
    /// The default context for single-tenant / un-authenticated requests.
    ///
    /// Returns `tenant_id = "public"`, `principal_id = None`,
    /// `plan_tier = Enterprise`. `Enterprise` because there are no limits
    /// on the internal default tenant — it's the server owner's own data.
    pub fn public() -> Self {
        Self {
            // SAFETY: "public" is 6 ASCII chars in [a-z], well within the
            // charset + length rules. unwrap never fires.
            tenant_id: TenantId::new("public").expect("'public' is a valid tenant id"),
            principal_id: None,
            plan_tier: PlanTier::Enterprise,
        }
    }
}

/// Shared validation for `TenantId` + `PrincipalId`.
///
/// Rules:
/// 1. Non-empty
/// 2. Byte length ≤ [`MAX_ID_LEN`]
/// 3. Charset: ASCII alphanumeric, `_`, `-` only. This rejects path-
///    traversal characters, SQL/shell meta, whitespace, null bytes, and
///    any non-ASCII input.
fn validate_id(raw: &str, field: &str) -> Result<()> {
    if raw.is_empty() {
        return Err(Error::Validation(format!("{field} cannot be empty")));
    }
    if raw.len() > MAX_ID_LEN {
        return Err(Error::Validation(format!(
            "{field} exceeds {MAX_ID_LEN} bytes"
        )));
    }
    if !raw
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Error::Validation(format!(
            "{field} contains invalid characters; only [A-Za-z0-9_-] allowed"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- TenantId validation ---------------------------------------------

    #[test]
    fn tenant_id_accepts_valid_inputs() {
        for raw in [
            "public",
            "acme",
            "acme-prod",
            "acme_prod",
            "a",
            "A1b2-C3_d4",
            "0",
            &"x".repeat(MAX_ID_LEN),
        ] {
            assert!(TenantId::new(raw).is_ok(), "expected {raw:?} to be valid");
        }
    }

    #[test]
    fn tenant_id_rejects_empty() {
        assert!(matches!(
            TenantId::new(""),
            Err(Error::Validation(msg)) if msg.contains("cannot be empty")
        ));
    }

    #[test]
    fn tenant_id_rejects_over_max_len() {
        let too_long = "x".repeat(MAX_ID_LEN + 1);
        assert!(matches!(
            TenantId::new(&too_long),
            Err(Error::Validation(msg)) if msg.contains("exceeds")
        ));
    }

    #[test]
    fn tenant_id_rejects_invalid_charset() {
        for raw in [
            " acme",            // leading space
            "acme ",            // trailing space
            "acme prod",        // embedded space
            "acme.prod",        // dot (path traversal seed)
            "acme/prod",        // slash
            "acme\\prod",       // backslash
            "acme:prod",        // colon (URL scheme seed)
            "acme'prod",        // single quote (SQL)
            "acme\"prod",       // double quote
            "acme;prod",        // statement separator
            "acme`prod",        // shell backtick
            "acme$prod",        // shell param
            "acme<script>",     // HTML
            "acme\0prod",       // null byte
            "\u{4E2D}\u{6587}", // unicode (Chinese)
            "acme\nprod",       // newline
            "acme\tprod",       // tab
        ] {
            assert!(
                matches!(
                    TenantId::new(raw),
                    Err(Error::Validation(msg)) if msg.contains("invalid characters")
                ),
                "expected {raw:?} to be rejected"
            );
        }
    }

    #[test]
    fn tenant_id_from_str_roundtrip() {
        let original = "acme-42";
        let parsed: TenantId = original.parse().expect("valid");
        assert_eq!(parsed.as_str(), original);
        assert_eq!(format!("{parsed}"), original);
    }

    #[test]
    fn tenant_id_serde_via_string() {
        let id = TenantId::new("acme").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"acme\"");
        let back: TenantId = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_str(), "acme");
    }

    #[test]
    fn tenant_id_serde_rejects_invalid() {
        let err = serde_json::from_str::<TenantId>("\"bad id\"").unwrap_err();
        assert!(err.to_string().contains("invalid characters"));
    }

    // ---- PrincipalId validation (mirrors TenantId) -----------------------

    #[test]
    fn principal_id_accepts_valid_inputs() {
        for raw in [
            "user-42",
            "svc_acme_ingest",
            "A1b2",
            &"x".repeat(MAX_ID_LEN),
        ] {
            assert!(PrincipalId::new(raw).is_ok(), "expected {raw:?} valid");
        }
    }

    #[test]
    fn principal_id_rejects_empty_and_overlong() {
        assert!(PrincipalId::new("").is_err());
        assert!(PrincipalId::new("x".repeat(MAX_ID_LEN + 1)).is_err());
    }

    #[test]
    fn principal_id_rejects_invalid_charset() {
        for raw in ["user 42", "user/42", "user.42", "\u{1F600}", "user\0"] {
            assert!(PrincipalId::new(raw).is_err(), "expected {raw:?} rejected");
        }
    }

    #[test]
    fn principal_id_from_str_roundtrip() {
        let p: PrincipalId = "svc_ingest".parse().expect("valid");
        assert_eq!(p.as_str(), "svc_ingest");
    }

    // ---- TenantContext::public -------------------------------------------

    #[test]
    fn public_context_is_public_tenant_enterprise_no_principal() {
        let ctx = TenantContext::public();
        assert_eq!(ctx.tenant_id.as_str(), "public");
        assert!(ctx.principal_id.is_none());
        assert_eq!(ctx.plan_tier, PlanTier::Enterprise);
    }

    #[test]
    fn tenant_context_is_clone() {
        // Forward-compat contract: 3b threads &TenantContext through
        // SearchPipeline::search. Clone must work so extractors can hand
        // out owned contexts per-request.
        let ctx = TenantContext::public();
        let dup = ctx.clone();
        assert_eq!(dup.tenant_id.as_str(), ctx.tenant_id.as_str());
    }

    // ---- PlanTier serde --------------------------------------------------

    #[test]
    fn plan_tier_serde_snake_case_roundtrip() {
        for (tier, expected) in [
            (PlanTier::Free, "\"free\""),
            (PlanTier::Pro, "\"pro\""),
            (PlanTier::Enterprise, "\"enterprise\""),
        ] {
            let json = serde_json::to_string(&tier).unwrap();
            assert_eq!(json, expected, "serialize {tier:?}");
            let back: PlanTier = serde_json::from_str(&json).unwrap();
            assert_eq!(back, tier, "deserialize {expected}");
        }
    }

    #[test]
    fn plan_tier_rejects_pascal_case() {
        assert!(serde_json::from_str::<PlanTier>("\"Free\"").is_err());
    }
}
