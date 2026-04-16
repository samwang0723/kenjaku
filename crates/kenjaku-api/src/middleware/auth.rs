//! Tenant auth middleware (Phase 3e — always on).
//!
//! Glues [`JwtValidator`] + [`TenantsCache`] into the live request path.
//! Every request goes through JWT validation — there is no disabled mode.
//!
//! # Resolution
//!
//! 1. `Authorization: Bearer <token>`:
//!    - validate JWT (generic `Unauthorized` on any failure -> 401
//!      `KNJK-4010`)
//!    - look up `claims.tenant_id` in [`TenantsCache`] -> 403
//!      `KNJK-4031` on miss
//!    - build [`TenantContext`] with `plan_tier` sourced from the DB
//!      row (NOT the claim — claim is advisory only)
//!    - insert into extensions, continue
//! 2. No Authorization header -> 401 `KNJK-4010`.
//! 3. `X-Tenant-Id` header is **IGNORED**. The middleware never reads
//!    it — defense against header-spoofing.
//!
//! # Span hygiene
//!
//! - `principal_id` is **hashed** before logging via
//!   [`hash_principal_for_log`] — 4 bytes of SHA-256, 8 hex chars.
//! - Raw `Authorization` header is **never** logged.
//! - `claims` are never logged.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::{info, warn};

use kenjaku_core::error::{AuthErrorCode, Error};
use kenjaku_core::types::tenant::{PrincipalId, TenantContext};
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::dto::response::ApiResponse;

/// Axum middleware fn. Wire via
/// `axum::middleware::from_fn_with_state(state.clone(), tenant_auth_middleware)`.
///
/// Phase 3e: every request goes through JWT validation. No disabled mode.
pub async fn tenant_auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let validator = &state.jwt_validator;

    let Some(bearer) = extract_bearer(req.headers()) else {
        // Don't log the (absent) header. Don't log the URL (could
        // be /search with a stale tenant hint). Just the code.
        warn!(
            error_code = AuthErrorCode::Unauthorized.code(),
            reason = "missing_authorization",
            "auth rejected"
        );
        return auth_error(AuthErrorCode::Unauthorized);
    };

    let claims = match validator.validate(bearer) {
        Ok(c) => c,
        Err(_) => {
            // `JwtValidator::validate` already flattens every
            // library error to `TenantAuth(Unauthorized)` and logs
            // the precise reason at `debug!` internally. Here we
            // just record the auth-failure code.
            warn!(
                error_code = AuthErrorCode::Unauthorized.code(),
                reason = "invalid_jwt",
                "auth rejected"
            );
            return auth_error(AuthErrorCode::Unauthorized);
        }
    };

    let Some(row) = state.tenants_cache.get(&claims.tenant_id) else {
        warn!(
            error_code = AuthErrorCode::TenantNotFound.code(),
            tenant_id = %claims.tenant_id,
            "auth rejected: tenant not in cache"
        );
        return auth_error(AuthErrorCode::TenantNotFound);
    };

    // `PlanTier::Suspended` reservation — 3c.2 defers the enum
    // variant + DB CHECK expansion to 3d. When that lands, insert:
    //   if matches!(row.plan_tier, PlanTier::Suspended) {
    //       return auth_error(AuthErrorCode::TenantSuspended);
    //   }
    // The KNJK-4032 code is already defined + tested in 3c.1.

    let tctx = TenantContext {
        tenant_id: claims.tenant_id,
        principal_id: claims.principal_id,
        // SECURITY: plan_tier comes from the DB row, NEVER from the
        // JWT claim. A compromised token issuer cannot escalate
        // privileges — tested by
        // `enabled_plan_tier_from_db_overrides_claim`.
        plan_tier: row.plan_tier,
    };

    info!(
        tenant_id = %tctx.tenant_id,
        plan_tier = ?tctx.plan_tier,
        principal_id_hash = %tctx.principal_id.as_ref().map(hash_principal_for_log).unwrap_or_default(),
        "auth ok"
    );
    req.extensions_mut().insert(tctx);
    next.run(req).await
}

/// Extract `Bearer <token>` from the `Authorization` header.
///
/// Returns `Some(token)` only when the header is well-formed ASCII
/// starting with the case-sensitive `"Bearer "` prefix and carries
/// a non-empty token body. Trimming prevents accidental whitespace
/// leaking into the validator.
///
/// The token string is **never logged** — caller receives it only to
/// hand straight to `JwtValidator::validate`.
fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    let hv = headers.get(header::AUTHORIZATION)?;
    let s = hv.to_str().ok()?;
    let token = s.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        return None;
    }
    Some(token)
}

/// Build the 401/403/etc. response for an [`AuthErrorCode`].
///
/// The body goes through `Error::user_message()` which yields the
/// generic public label (e.g. "Unauthorized tenant"). The KNJK code
/// travels in a response header `x-knjk-error-code` for operator
/// log correlation — it deliberately is NOT in the body so a
/// compromised log pipeline can still tie responses to error codes
/// without parsing JSON.
fn auth_error(code: AuthErrorCode) -> Response {
    let err = Error::TenantAuth(code);
    let status = match code.http_status() {
        401 => StatusCode::UNAUTHORIZED,
        403 => StatusCode::FORBIDDEN,
        429 => StatusCode::TOO_MANY_REQUESTS,
        503 => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let body = ApiResponse::<()>::err(err.user_message().to_string());
    let mut resp = (status, axum::Json(body)).into_response();
    resp.headers_mut().insert(
        "x-knjk-error-code",
        code.code().parse().expect("KNJK codes are ASCII"),
    );
    resp
}

/// Hash a `PrincipalId` for safe logging.
///
/// Returns the first 8 hex chars of `SHA-256(principal_id)` — 4
/// bytes of entropy. This is a **correlation tool** for operator
/// tracing, NOT a rainbow-table defense. There's no salt because the
/// goal is cross-request stability of the hash, not cryptographic
/// unlinkability.
///
/// **Every** span/log statement that wants to include principal_id
/// MUST go through this helper. Code review invariant.
pub(crate) fn hash_principal_for_log(p: &PrincipalId) -> String {
    let mut h = Sha256::new();
    h.update(p.as_str().as_bytes());
    let out = h.finalize();
    hex::encode(&out[..4])
}

// Note: `router.rs` wires this middleware via
// `axum::middleware::from_fn_with_state(state.clone(), tenant_auth_middleware)`
// directly. A dedicated `auth_layer` helper would have to spell out
// the long Axum return type — the call-site spelling is shorter.

// =====================================================================
// Tests — TDD order (middleware-security-critical tests FIRST)
// =====================================================================
//
// Strategy:
// - In-memory TenantsCache populated via `from_map`.
// - Stub `JwtValidator` wrapping a real one built against a fixed
//   test keypair — deterministic + fast, same pattern as 3c.1 unit
//   tests. No fake Validator trait needed.
// - Build a minimal router with the middleware wired and a reflector
//   handler that reads the `TenantContext` extension and returns its
//   tenant_id + plan_tier in the body.
// - Drive requests via `tower::ServiceExt::oneshot` — no live HTTP,
//   no TCP listener.

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::Router;
    use axum::body::Body;
    use axum::body::to_bytes;
    use std::sync::OnceLock;

    use axum::extract::Extension;
    use axum::http::Method;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use jsonwebtoken::{Algorithm, EncodingKey, Header as JwtHeader, encode as jwt_encode};
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use tower::ServiceExt;

    use kenjaku_core::config::{JwtAlgorithm, JwtConfig};
    use kenjaku_core::types::tenant::{PlanTier, TenantContext, TenantId};
    use kenjaku_infra::auth::JwtValidator;
    use kenjaku_infra::postgres::{TenantRow, TenantsCache};

    use crate::extractors::TenantCtx;

    // ---------- Test fixtures ---------------------------------------------
    //
    // Historically (3c.1/3c.2) these tests embedded a hardcoded
    // RSA-2048 PEM pair. PR #17 Copilot review #5 flagged that as a
    // CWE-798 hardcoded-credentials smell. Migrated to runtime keygen
    // matching the pattern already in `tests/auth_flow.rs` — ~200ms
    // keygen cost amortized across all tests in this module via
    // `OnceLock`. No credentials in the source tree.

    /// Process-wide test keypair. Generated lazily via seeded RNG so
    /// the materialized key is deterministic + reproducible across
    /// runs, and the per-process cost is paid at most once.
    struct TestKeypair {
        private_pem: String,
        public_pem: String,
    }

    fn keypair() -> &'static TestKeypair {
        static KP: OnceLock<TestKeypair> = OnceLock::new();
        KP.get_or_init(|| {
            // Seed is arbitrary — deterministic fixture, not sensitive
            // material. Mirrors `tests/auth_flow.rs` seed.
            let mut rng = StdRng::seed_from_u64(0xDEADBEEF_CAFEBABE);
            let priv_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
            let pub_key = priv_key.to_public_key();
            let private_pem = priv_key
                .to_pkcs8_pem(LineEnding::LF)
                .expect("pkcs8 priv pem")
                .to_string();
            let public_pem = pub_key
                .to_public_key_pem(LineEnding::LF)
                .expect("spki pub pem");
            TestKeypair {
                private_pem,
                public_pem,
            }
        })
    }

    const TEST_ISSUER: &str = "kenjaku-test-issuer";
    const TEST_AUDIENCE: &str = "kenjaku-test-audience";

    fn make_validator() -> Arc<JwtValidator> {
        let cfg = JwtConfig {
            issuer: TEST_ISSUER.into(),
            audience: TEST_AUDIENCE.into(),
            public_key_path: "<test>".into(),
            algorithm: JwtAlgorithm::RS256,
            clock_skew_secs: 5,
        };
        Arc::new(JwtValidator::new(&cfg, keypair().public_pem.as_bytes()).unwrap())
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn mint_token(claims: serde_json::Value) -> String {
        let key = EncodingKey::from_rsa_pem(keypair().private_pem.as_bytes()).unwrap();
        jwt_encode(&JwtHeader::new(Algorithm::RS256), &claims, &key).unwrap()
    }

    fn valid_claims(tenant_id: &str, plan: &str) -> serde_json::Value {
        let now = now_secs();
        serde_json::json!({
            "tenant_id": tenant_id,
            "principal_id": "user-9",
            "plan_tier": plan,
            "exp": now + 300,
            "iat": now,
            "iss": TEST_ISSUER,
            "aud": TEST_AUDIENCE,
        })
    }

    fn cache_with(rows: &[(&str, PlanTier)]) -> Arc<TenantsCache> {
        let mut m = HashMap::new();
        for (id, plan) in rows {
            let tid = TenantId::new(*id).unwrap();
            m.insert(
                tid.clone(),
                Arc::new(TenantRow {
                    id: tid,
                    name: format!("{id} Corp"),
                    plan_tier: *plan,
                    config_overrides: serde_json::json!({}),
                }),
            );
        }
        Arc::new(TenantsCache::from_map(m))
    }

    fn test_jwt_config() -> JwtConfig {
        JwtConfig {
            issuer: TEST_ISSUER.into(),
            audience: TEST_AUDIENCE.into(),
            public_key_path: "<test>".into(),
            algorithm: JwtAlgorithm::RS256,
            clock_skew_secs: 5,
        }
    }

    // Small typed state used only in tests. Matches the subset of
    // `AppState` fields the middleware reads. Crucially NOT using
    // the real `AppState` — building one needs SearchService etc.
    // which aren't relevant here. The middleware fn signature takes
    // `State<Arc<AppState>>` so we instead wrap it in a trait-like
    // extractor pattern via `Extension`.
    //
    // For these tests we build a tiny router with an inline
    // middleware that calls straight into the decision logic via a
    // test shim with identical semantics.

    /// Subset of the middleware state used by tests. Identical
    /// decision logic as `tenant_auth_middleware`; calls the same
    /// helpers. Phase 3e: no disabled branch — every request goes
    /// through JWT validation.
    async fn test_auth_middleware(
        Extension(fx): Extension<TestFixture>,
        mut req: Request,
        next: Next,
    ) -> Response {
        let validator = &fx.validator;
        let Some(bearer) = extract_bearer(req.headers()) else {
            return auth_error(AuthErrorCode::Unauthorized);
        };
        let claims = match validator.validate(bearer) {
            Ok(c) => c,
            Err(_) => return auth_error(AuthErrorCode::Unauthorized),
        };
        let Some(row) = fx.cache.get(&claims.tenant_id) else {
            return auth_error(AuthErrorCode::TenantNotFound);
        };
        let tctx = TenantContext {
            tenant_id: claims.tenant_id,
            principal_id: claims.principal_id,
            plan_tier: row.plan_tier,
        };
        req.extensions_mut().insert(tctx);
        next.run(req).await
    }

    #[derive(Clone)]
    struct TestFixture {
        cache: Arc<TenantsCache>,
        validator: Arc<JwtValidator>,
    }

    async fn reflector(TenantCtx(tctx): TenantCtx) -> String {
        format!(
            "{}:{}:{}",
            tctx.tenant_id,
            tctx.principal_id
                .as_ref()
                .map(|p| p.as_str().to_string())
                .unwrap_or_else(|| "-".into()),
            match tctx.plan_tier {
                PlanTier::Free => "free",
                PlanTier::Pro => "pro",
                PlanTier::Enterprise => "enterprise",
            }
        )
    }

    fn app(fx: TestFixture) -> Router {
        Router::new()
            .route("/reflect", get(reflector))
            .layer(axum::middleware::from_fn(test_auth_middleware))
            .layer(Extension(fx))
    }

    async fn response_body_string(resp: Response) -> String {
        // axum 0.8 — `Body::to_bytes` via `axum::body::to_bytes`.
        // Cap at 64 KiB to match the production request_body limit.
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    // ---------- TDD middleware tests (Phase 3e: always on) -----------------

    /// Test 1 — valid JWT + known tenant -> 200 with
    /// the tenant_id/principal_id from the claim and plan_tier from
    /// the DB row.
    #[tokio::test]
    async fn test_enabled_valid_jwt_resolves_tenant_context() {
        let fx = TestFixture {
            cache: cache_with(&[("acme", PlanTier::Pro)]),
            validator: make_validator(),
        };
        let token = mint_token(valid_claims("acme", "pro"));
        let req = HttpRequest::builder()
            .method(Method::GET)
            .uri("/reflect")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app(fx).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = response_body_string(resp).await;
        assert_eq!(body, "acme:user-9:pro");
    }

    /// Test 4 — `enabled=true`, claim says Enterprise, DB row says
    /// Free → context carries Free (DB is authoritative). This is
    /// the privilege-escalation defense.
    #[tokio::test]
    async fn test_enabled_plan_tier_from_db_overrides_claim() {
        let fx = TestFixture {
            cache: cache_with(&[("acme", PlanTier::Free)]), // DB = Free
            validator: make_validator(),
        };
        // Claim says Enterprise. Must be ignored.
        let token = mint_token(valid_claims("acme", "enterprise"));
        let req = HttpRequest::builder()
            .method(Method::GET)
            .uri("/reflect")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app(fx).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = response_body_string(resp).await;
        assert_eq!(
            body, "acme:user-9:free",
            "SECURITY: plan_tier must source from DB, not claim"
        );
    }

    /// Test 5 — `enabled=true` + no Authorization header → 401.
    #[tokio::test]
    async fn test_enabled_missing_authorization_yields_4010() {
        let fx = TestFixture {
            cache: cache_with(&[("acme", PlanTier::Pro)]),
            validator: make_validator(),
        };
        let req = HttpRequest::builder()
            .method(Method::GET)
            .uri("/reflect")
            .body(Body::empty())
            .unwrap();
        let resp = app(fx).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 401);
        let code = resp
            .headers()
            .get("x-knjk-error-code")
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        assert_eq!(code.as_deref(), Some("KNJK-4010"));
    }

    /// Test 6 — `enabled=true` + valid JWT + tenant not in cache → 403.
    #[tokio::test]
    async fn test_enabled_unknown_tenant_yields_4031() {
        let fx = TestFixture {
            cache: cache_with(&[("other", PlanTier::Pro)]), // no "acme"
            validator: make_validator(),
        };
        let token = mint_token(valid_claims("acme", "pro"));
        let req = HttpRequest::builder()
            .method(Method::GET)
            .uri("/reflect")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app(fx).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 403);
        let code = resp
            .headers()
            .get("x-knjk-error-code")
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        assert_eq!(code.as_deref(), Some("KNJK-4031"));
    }

    /// Test 7 — `enabled=true` + `X-Tenant-Id` header only (no JWT)
    /// → 401. The header is NEVER the trust anchor when tenancy is
    /// enforced. This is the header-spoofing defense (callback to PR
    /// #16 #6 rejection rationale).
    #[tokio::test]
    async fn test_enabled_ignores_x_tenant_id_header() {
        let fx = TestFixture {
            cache: cache_with(&[("victim", PlanTier::Enterprise)]),
            validator: make_validator(),
        };
        // Attacker sets X-Tenant-Id but supplies no JWT. Must 401.
        let req = HttpRequest::builder()
            .method(Method::GET)
            .uri("/reflect")
            .header("x-tenant-id", "victim")
            .body(Body::empty())
            .unwrap();
        let resp = app(fx).oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            401,
            "SECURITY: X-Tenant-Id must NEVER grant access when tenancy is enabled"
        );
    }

    // ---------- Principal hashing ----------------------------------------

    #[test]
    fn hash_principal_for_log_is_deterministic_and_short() {
        let p = PrincipalId::new("user-abc").unwrap();
        let a = hash_principal_for_log(&p);
        let b = hash_principal_for_log(&p);
        assert_eq!(a, b, "hash must be deterministic across calls");
        assert_eq!(a.len(), 8, "4 bytes hex-encoded = 8 chars");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, "user-abc", "hash must NOT equal raw principal id");
    }

    #[test]
    fn hash_principal_for_log_differs_between_principals() {
        let a = hash_principal_for_log(&PrincipalId::new("user-1").unwrap());
        let b = hash_principal_for_log(&PrincipalId::new("user-2").unwrap());
        assert_ne!(a, b);
    }

    // ---------- Bearer extraction ----------------------------------------

    #[test]
    fn extract_bearer_happy_path() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer abc.def.ghi".parse().unwrap());
        assert_eq!(extract_bearer(&h), Some("abc.def.ghi"));
    }

    #[test]
    fn extract_bearer_rejects_malformed() {
        for (name, val) in [
            ("empty", ""),
            ("wrong_scheme", "Basic dXNlcjpwYXNz"),
            ("lowercase_bearer", "bearer abc.def.ghi"), // case-sensitive
            ("bearer_only", "Bearer "),
            ("bearer_whitespace_only", "Bearer    "),
        ] {
            let mut h = HeaderMap::new();
            if !val.is_empty() {
                h.insert("authorization", val.parse().unwrap());
            }
            assert!(
                extract_bearer(&h).is_none(),
                "extract_bearer must reject {name:?}={val:?}"
            );
        }
    }
}
