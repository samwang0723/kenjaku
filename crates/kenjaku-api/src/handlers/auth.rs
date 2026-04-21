//! `POST /api/v1/auth/login` — email/password → JWT.
//!
//! Runs **pre-auth** (outside the `tenant_auth_middleware` layer) —
//! by definition there's no tenant context yet. The handler resolves
//! the tenant from the user row, verifies the tenant exists in the
//! `TenantsCache`, mints a JWT via [`JwtMinter`], and returns it.
//!
//! # Uniform failure
//!
//! Every failure path — unknown email, disabled user, missing tenant —
//! returns the same `401 InvalidCredentials` body. The handler never
//! branches on which condition tripped the check.
//!
//! # Password hygiene
//!
//! `#[instrument(skip(password))]` keeps the plaintext out of tracing
//! spans. The handler never logs the body.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, response::Response};
use tracing::{debug, instrument, warn};

use kenjaku_core::error::{AuthErrorCode, Error};
use kenjaku_core::types::tenant::{PrincipalId, TenantId};

use crate::AppState;
use crate::dto::auth::{LoginRequest, LoginResponse};
use crate::dto::response::ApiResponse;

/// Maximum accepted email / password length on the login form. Argon2
/// verification is bounded by the stored hash, not the input length —
/// these limits are UX + cheap DoS guards.
pub(crate) const MAX_LOGIN_EMAIL_LEN: usize = 320;
pub(crate) const MAX_LOGIN_PASSWORD_LEN: usize = 1024;

/// `POST /api/v1/auth/login`.
///
/// Handler is intentionally short — all branching distills into the
/// [`InvalidCredentials`] return. The three distinct reasons (unknown
/// email, disabled user, password mismatch) are logged at `debug` but
/// never surfaced.
#[instrument(skip(state, body), fields(email = %body.email))]
pub async fn login(State(state): State<Arc<AppState>>, Json(body): Json<LoginRequest>) -> Response {
    if body.email.is_empty() || body.email.len() > MAX_LOGIN_EMAIL_LEN {
        return auth_error_response(AuthErrorCode::InvalidCredentials);
    }
    if body.password.is_empty() || body.password.len() > MAX_LOGIN_PASSWORD_LEN {
        return auth_error_response(AuthErrorCode::InvalidCredentials);
    }

    // Verify the (email, password) pair. `verify_password` already
    // flattens miss/disabled/wrong-pw into InvalidCredentials.
    let user = match state
        .users_repo
        .verify_password(&body.email, &body.password)
        .await
    {
        Ok(u) => u,
        Err(Error::TenantAuth(code)) => return auth_error_response(code),
        Err(e) => {
            warn!(error = %e, "login: verify_password unexpected error");
            return auth_error_response(AuthErrorCode::InvalidCredentials);
        }
    };

    // Tenant must exist in the cache (post-3e invariant). A user row
    // could orphan past its tenant's removal via an admin CLI race —
    // return InvalidCredentials rather than leaking that the user
    // exists but the tenant is gone.
    let tid = match TenantId::new(user.tenant_id.clone()) {
        Ok(t) => t,
        Err(_) => {
            warn!(tenant_id = %user.tenant_id, "login: user.tenant_id fails validation");
            return auth_error_response(AuthErrorCode::InvalidCredentials);
        }
    };
    let Some(tenant_row) = state.tenants_cache.get(&tid) else {
        debug!(tenant_id = %tid, "login: tenant missing from cache");
        return auth_error_response(AuthErrorCode::InvalidCredentials);
    };

    // Principal id = user UUID (as a PrincipalId-compatible slug).
    // The newtype rejects any UUID that happens to include `/` (never
    // — `-` is allowlisted in tenant::MAX_ID_LEN charset), but we
    // still surface the construction result cleanly.
    let pid = match PrincipalId::new(user.id.to_string()) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "login: principal_id construction failed");
            return internal_error_response();
        }
    };

    // Mint the token. `ttl=None` uses JwtConfig.ttl_seconds.
    let (token, exp) =
        match state
            .jwt_minter
            .mint(&tid, Some(&pid), Some(tenant_row.plan_tier), None)
        {
            Ok(pair) => pair,
            Err(e) => {
                warn!(error = %e, "login: JwtMinter::mint failed");
                return internal_error_response();
            }
        };

    // Fire-and-forget last_login stamp — no reason to block the
    // response on it, and a failure here just means a stale timestamp.
    if let Err(e) = state.users_repo.touch_last_login(user.id).await {
        warn!(error = %e, "login: touch_last_login failed (non-fatal)");
    }

    let resp = LoginResponse {
        token,
        expires_at: exp.to_rfc3339(),
        tenant_id: user.tenant_id,
        email: user.email,
        role: user.role.as_str().to_string(),
    };
    (StatusCode::OK, Json(ApiResponse::ok(resp))).into_response()
}

/// Build a response for an auth-error code. Mirrors the helper in
/// `middleware::auth::auth_error` but scoped to this module's two
/// use cases (401 InvalidCredentials).
pub(crate) fn auth_error_response(code: AuthErrorCode) -> Response {
    let err = Error::TenantAuth(code);
    let status = match code.http_status() {
        401 => StatusCode::UNAUTHORIZED,
        403 => StatusCode::FORBIDDEN,
        429 => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let body = ApiResponse::<()>::err(err.user_message().to_string());
    let mut resp = (status, Json(body)).into_response();
    resp.headers_mut().insert(
        "x-knjk-error-code",
        code.code().parse().expect("KNJK codes are ASCII"),
    );
    resp
}

fn internal_error_response() -> Response {
    let body = ApiResponse::<()>::err("Internal server error".to_string());
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
}
