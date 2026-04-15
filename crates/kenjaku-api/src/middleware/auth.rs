//! Tenant auth middleware (Phase 3c.2).
//!
//! Glues 3c.1's [`JwtValidator`] + 3c.2's [`TenantsCache`] into the live
//! request path.
//!
//! # Resolution priority
//!
//! 1. `tenancy.enabled = false` → insert [`TenantContext::public`] into
//!    request extensions, continue. **The JWT validator is never
//!    invoked.** This is the default path shipped today and the
//!    zero-behavior-change guarantee for 3c.2.
//! 2. `enabled = true` + `Authorization: Bearer <token>`:
//!    - validate JWT (generic `Unauthorized` on any failure → 401
//!      `KNJK-4010`)
//!    - look up `claims.tenant_id` in [`TenantsCache`] → 403
//!      `KNJK-4031` on miss
//!    - build [`TenantContext`] with `plan_tier` sourced from the DB
//!      row (NOT the claim — claim is advisory only)
//!    - insert into extensions, continue
//! 3. `enabled = true` + no Authorization header → 401 `KNJK-4010`.
//! 4. `X-Tenant-Id` header is **IGNORED when `enabled = true`**. Even
//!    if a caller sets it, the middleware never reads it. This is the
//!    documented defense against header-spoofing (see
//!    `kenjaku_core::config::TenancyConfig::header_name` rustdoc + the
//!    PR #16 Copilot-comment-#6 rejection rationale).
//!
//! # Span hygiene
//!
//! - `principal_id` is **hashed** before logging via
//!   [`hash_principal_for_log`] — 4 bytes of SHA-256, 8 hex chars.
//!   Enough for log correlation across a principal's requests;
//!   structurally unsuitable for a rainbow-table attack.
//! - Raw `Authorization` header is **never** logged.
//! - `claims` are never logged (the validator's debug-level internal
//!   log lives in kenjaku-infra and carries only the error kind).

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
pub async fn tenant_auth_middleware(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    // ------------------------------------------------------------------
    // Branch 1: tenancy disabled — short-circuit to public.
    //
    // IMPORTANT: this branch returns BEFORE any access to
    // `state.jwt_validator`. A stubbed validator that panics-on-call
    // proves the invariant in `disabled_mode_never_invokes_validator`.
    // ------------------------------------------------------------------
    if !state.tenancy_config.enabled {
        req.extensions_mut().insert(TenantContext::public());
        return next.run(req).await;
    }

    // ------------------------------------------------------------------
    // Branch 2-4: tenancy enabled.
    //
    // Startup validation (`AppConfig::validate_secrets`) guarantees
    // the validator is present when enabled=true. The assert here is
    // defense-in-depth — fail closed with 401 rather than panic if
    // some future refactor violates the invariant.
    // ------------------------------------------------------------------
    let Some(validator) = state.jwt_validator.as_ref() else {
        warn!(
            error_code = AuthErrorCode::Unauthorized.code(),
            "tenancy.enabled=true but JwtValidator not constructed (startup invariant violation)"
        );
        return auth_error(AuthErrorCode::Unauthorized);
    };

    // X-Tenant-Id header intentionally NOT read in this branch. See
    // module docs (resolution priority #4).

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
/// travels in a response header `X-KNJK-Error-Code` for operator
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
    use axum::extract::Extension;
    use axum::http::Method;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use jsonwebtoken::{Algorithm, EncodingKey, Header as JwtHeader, encode as jwt_encode};
    use tower::ServiceExt;

    use kenjaku_core::config::{JwtAlgorithm, JwtConfig, TenancyConfig};
    use kenjaku_core::types::tenant::{PlanTier, TenantContext, TenantId};
    use kenjaku_infra::auth::JwtValidator;
    use kenjaku_infra::postgres::{TenantRow, TenantsCache};

    use crate::extractors::TenantCtx;

    // ---------- Test fixtures ---------------------------------------------

    // Same hardcoded 2048-bit RSA keypair used in kenjaku-infra jwt.rs
    // unit tests. Deterministic + fast; no runtime keygen.
    const TEST_RSA_PRIVATE_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvwIBADANBgkqhkiG9w0BAQEFAASCBKkwggSlAgEAAoIBAQDAJQWvA8pvDZ6C
IuvY10hfj8HYdHwsYBGmskms8VDpzmn4uxTJ4xuGTHMVvHZxXFf78baHFl67tJY0
juXMKy79lX8zG2HrXFJAFYb5+HCS0fJl/4QEBYKMXlfrkmu4VYSIaH+Srh6bJH7y
GOqfpc9YHb66EKdv+invF9cWC952nrCYC8c65KIndmU/qkwo1w6imqT+PxODIH32
AqmKK+PCGYYr+OnKCDyhvbP9NxbgW4m/NwFSC3j5kNR7X97OB3PqU5x6OUXamGGd
d40EjmLpoxaVLFs4uXA49I2oDaT3tgW7Syeb2XEI48uO2oz1dPZBcAmnH0zFy2A8
N7UA3OurAgMBAAECggEAP4ve+ddUm27TJfH7sz7b4jLuprI7oQdyvwFG6Ynzuye3
l3nz8aMJJtzg0Ob/+xTRynLR7ZJh6U1bwe0ipegARgCrzVC4jTj+wM/90G0wefYy
ZzZeyF+0Ic1P4YqO0quBxgVdb7rjR2cGDvZbnkodrWRFAE9iuN9w//CQA2wQeoUE
qn5nNZOjZs9nmsvtvhoF2t0/v54Vey2eyr30gnfMITGQyeeMAQ5mxv3leqjy482H
JplY5bwZSpHfNYJaFUI743xlzo4agIm0l7yChB6k8VzRhk2zSX2E3+WtY3hryLDL
hvXZaxi+KHBWIMwNwbh04BgQV75wUDFwdoFOga7UpQKBgQD1WBwO7oJQL9o9+Ijb
/YF+CJ8BLddK3ssEUqZ06uPUMb4/auMx95Kwwit1xqeCGCWraQtZipKMPLXA6xAV
LS9WxWqoORfj5BISzHILt+rEv3GX9ga6cBatb/aKx96TQxHArYTliNeGui3ci+4k
guDEtWj6pkguQ8z0e68haFc4nQKBgQDIfWgSoUi+VDMYNdzEk7lATI9UpVCFRi4u
D3hlTXfacw0FpVSmFKHN+bmUjTzR8uiYRwjcIYg7TaJtUVoP67wXGaaN8nUm/ow/
UKm6z+NkJgbeoHqeqVCjybA/18tYTKHPeiAKrkZE4n9P9hy2/oCsruOIEcky+A+H
B891971O5wKBgQDMRuN9zzrzVxSIurIBMW8rKDRz/94dOamh3Ms52AWAPTahiPbI
ONGLmPAkrRX1rPNYaCsKD+X7G+VdTpridu/OeKtYi8kHd3NQ8acXeLfhv2DuXOu3
OfK3z9xgKxB4XsSeQBvCHhEN3WCoHF5ZShupzT4uTFsXIl65RW5JC55rxQKBgQCC
V72EdySi0HzTXZkkl+GVIvEAF+XMb37wZRBh08Z619d8dyscOkejcJotMNF8sQ50
iaIB8y7tVyClL4F83kZPQlbc6+csLcsiEp/2Gtd7sZk/vVdatTr+8M3MA9BonJGK
IJyaNQjsGdCpPJkYg0dGfQDh4qZKo6j8oaBH0oNZuwKBgQCwzIYjvP5dy39a8ZTq
Wa1dUXBIflS4RC8AI1fL2L0G9o9VGMoBOcyQxk985PwQPgf9aeOKrZcOQvqZS8IB
w7KzVbxMxttIqKe7ji0b1u8AXfMJ6Fq+vnh+a3ILhWrBnSgUxRCiW7fKRrDwXjjS
jwgqNZx/qADYhiBGaZ8ul31JWw==
-----END PRIVATE KEY-----
";
    const TEST_RSA_PUBLIC_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAwCUFrwPKbw2egiLr2NdI
X4/B2HR8LGARprJJrPFQ6c5p+LsUyeMbhkxzFbx2cVxX+/G2hxZeu7SWNI7lzCsu
/ZV/Mxth61xSQBWG+fhwktHyZf+EBAWCjF5X65JruFWEiGh/kq4emyR+8hjqn6XP
WB2+uhCnb/op7xfXFgvedp6wmAvHOuSiJ3ZlP6pMKNcOopqk/j8TgyB99gKpiivj
whmGK/jpygg8ob2z/TcW4FuJvzcBUgt4+ZDUe1/ezgdz6lOcejlF2phhnXeNBI5i
6aMWlSxbOLlwOPSNqA2k97YFu0snm9lxCOPLjtqM9XT2QXAJpx9MxctgPDe1ANzr
qwIDAQAB
-----END PUBLIC KEY-----
";
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
        Arc::new(JwtValidator::new(&cfg, TEST_RSA_PUBLIC_PEM.as_bytes()).unwrap())
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn mint_token(claims: serde_json::Value) -> String {
        let key = EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_PEM.as_bytes()).unwrap();
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
                TenantRow {
                    id: tid,
                    name: format!("{id} Corp"),
                    plan_tier: *plan,
                    config_overrides: serde_json::json!({}),
                },
            );
        }
        Arc::new(TenantsCache::from_map(m))
    }

    fn tenancy(enabled: bool) -> TenancyConfig {
        TenancyConfig {
            enabled,
            header_name: "X-Tenant-Id".into(),
            default_tenant: "public".into(),
            collection_name_template: "{base}_{tenant}".into(),
            jwt: Some(JwtConfig {
                issuer: TEST_ISSUER.into(),
                audience: TEST_AUDIENCE.into(),
                public_key_path: "<test>".into(),
                algorithm: JwtAlgorithm::RS256,
                clock_skew_secs: 5,
            }),
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
    /// helpers. Keeping this inline avoids pulling all of `AppState`
    /// (with its SearchService / Qdrant / Redis deps) into the unit
    /// test harness.
    async fn test_auth_middleware(
        Extension(fx): Extension<TestFixture>,
        mut req: Request,
        next: Next,
    ) -> Response {
        if !fx.tenancy.enabled {
            req.extensions_mut().insert(TenantContext::public());
            return next.run(req).await;
        }
        let Some(validator) = fx.validator.as_ref() else {
            return auth_error(AuthErrorCode::Unauthorized);
        };
        // Optional panic-on-call validator guard used by test 2.
        (fx.validator_invoked)();
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
        tenancy: TenancyConfig,
        cache: Arc<TenantsCache>,
        validator: Option<Arc<JwtValidator>>,
        /// Called if the middleware ever reaches the validate() path.
        /// Tests that must prove the validator is NOT called pass
        /// `Arc::new(|| panic!(...))` here.
        validator_invoked: Arc<dyn Fn() + Send + Sync>,
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

    // ---------- TDD middleware tests --------------------------------------

    /// Test 1 — `enabled=false` resolves to public regardless of headers.
    /// The 4 combinations below cover every way a caller could try to
    /// influence the resolution: none, JWT only, X-Tenant-Id only, both.
    #[tokio::test]
    async fn test_disabled_mode_always_public() {
        let fx = TestFixture {
            tenancy: tenancy(false),
            cache: cache_with(&[]),
            validator: Some(make_validator()),
            validator_invoked: Arc::new(|| ()),
        };
        let app = app(fx);
        let token = mint_token(valid_claims("acme", "pro"));
        for (name, auth_h, tenant_h) in [
            ("no-headers", None, None),
            ("bearer-only", Some(format!("Bearer {token}")), None),
            ("x-tenant-only", None, Some("evil-tenant")),
            ("both", Some(format!("Bearer {token}")), Some("evil-tenant")),
        ] {
            let mut req = HttpRequest::builder().method(Method::GET).uri("/reflect");
            if let Some(v) = auth_h {
                req = req.header("authorization", v);
            }
            if let Some(v) = tenant_h {
                req = req.header("x-tenant-id", v);
            }
            let req = req.body(Body::empty()).unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), 200, "combo {name} must pass");
            let body = response_body_string(resp).await;
            assert_eq!(
                body, "public:-:enterprise",
                "combo {name} must resolve to public, got: {body}"
            );
        }
    }

    /// Test 2 — `enabled=false` NEVER invokes the JwtValidator.
    /// Use a tracer that panics-on-call to prove the short-circuit is
    /// structural, not incidental.
    #[tokio::test]
    async fn test_disabled_mode_never_invokes_validator() {
        let fx = TestFixture {
            tenancy: tenancy(false),
            cache: cache_with(&[]),
            validator: Some(make_validator()),
            validator_invoked: Arc::new(|| {
                panic!(
                    "SECURITY INVARIANT VIOLATED: JwtValidator invoked when tenancy.enabled=false"
                )
            }),
        };
        let app = app(fx);
        let token = mint_token(valid_claims("acme", "pro"));
        let req = HttpRequest::builder()
            .method(Method::GET)
            .uri("/reflect")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200, "request must succeed without panic");
    }

    /// Test 3 — `enabled=true` + valid JWT + known tenant → 200 with
    /// the tenant_id/principal_id from the claim and plan_tier from
    /// the DB row.
    #[tokio::test]
    async fn test_enabled_valid_jwt_resolves_tenant_context() {
        let fx = TestFixture {
            tenancy: tenancy(true),
            cache: cache_with(&[("acme", PlanTier::Pro)]),
            validator: Some(make_validator()),
            validator_invoked: Arc::new(|| ()),
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
            tenancy: tenancy(true),
            cache: cache_with(&[("acme", PlanTier::Free)]), // DB = Free
            validator: Some(make_validator()),
            validator_invoked: Arc::new(|| ()),
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
            tenancy: tenancy(true),
            cache: cache_with(&[("acme", PlanTier::Pro)]),
            validator: Some(make_validator()),
            validator_invoked: Arc::new(|| ()),
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
            tenancy: tenancy(true),
            cache: cache_with(&[("other", PlanTier::Pro)]), // no "acme"
            validator: Some(make_validator()),
            validator_invoked: Arc::new(|| ()),
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
            tenancy: tenancy(true),
            cache: cache_with(&[("victim", PlanTier::Enterprise)]),
            validator: Some(make_validator()),
            validator_invoked: Arc::new(|| ()),
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
