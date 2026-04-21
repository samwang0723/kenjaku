//! `require_admin` middleware — gates `/api/v1/admin/*` routes.
//!
//! Runs AFTER `tenant_auth_middleware` (so `TenantContext` is already
//! in extensions). Reads `tctx.principal_id`, looks up the user row
//! via the `tenant_id`-scoped `find_by_id`, and returns
//! `403 AdminForbidden` when the user is missing, disabled, or not an
//! `admin`.
//!
//! # Layering
//!
//! ```text
//! Request
//!   → tenant_auth_middleware       (JWT → TenantContext, 401 on failure)
//!   → require_admin                (user.role = admin?, 403 on failure)
//!   → rate-limit
//!   → admin handler
//! ```
//!
//! `tenant_id` scoping is built into `find_by_id` so a tenant-A
//! admin's JWT cannot validate against a tenant-B user row even if
//! the `principal_id` happens to exist there.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::{debug, warn};
use uuid::Uuid;

use kenjaku_core::error::{AuthErrorCode, Error};
use kenjaku_core::types::tenant::TenantContext;
use kenjaku_infra::postgres::Role;

use crate::AppState;
use crate::dto::response::ApiResponse;

pub async fn require_admin(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let tctx = match req.extensions().get::<TenantContext>().cloned() {
        Some(t) => t,
        None => {
            // Programmer error: require_admin mounted outside the
            // tenant_auth_middleware layer. Fail closed.
            warn!("require_admin: no TenantContext in extensions (middleware order bug?)");
            return auth_error_response(AuthErrorCode::AdminForbidden);
        }
    };

    let Some(principal) = tctx.principal_id.as_ref() else {
        // Service tokens that carry no principal aren't admins by
        // definition — the admin UI only issues user-principal tokens.
        debug!(tenant_id = %tctx.tenant_id, "require_admin: token has no principal_id");
        return auth_error_response(AuthErrorCode::AdminForbidden);
    };

    let Ok(principal_uuid) = Uuid::parse_str(principal.as_str()) else {
        debug!(principal_raw = %principal, "require_admin: principal_id not a UUID");
        return auth_error_response(AuthErrorCode::AdminForbidden);
    };

    let row = match state
        .users_repo
        .find_by_id(principal_uuid, tctx.tenant_id.as_str())
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            // User not in cache for this tenant — either deleted or
            // belongs to a different tenant. Either way: not admin.
            debug!(tenant_id = %tctx.tenant_id, "require_admin: user row missing");
            return auth_error_response(AuthErrorCode::AdminForbidden);
        }
        Err(e) => {
            warn!(error = %e, "require_admin: find_by_id failed");
            return auth_error_response(AuthErrorCode::AdminForbidden);
        }
    };

    if !row.enabled {
        debug!(tenant_id = %tctx.tenant_id, "require_admin: user disabled");
        return auth_error_response(AuthErrorCode::AdminForbidden);
    }

    if !matches!(row.role, Role::Admin) {
        debug!(tenant_id = %tctx.tenant_id, role = ?row.role, "require_admin: user is not admin");
        return auth_error_response(AuthErrorCode::AdminForbidden);
    }

    next.run(req).await
}

fn auth_error_response(code: AuthErrorCode) -> Response {
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    // Full decision-tree behavior (happy admin, member reject,
    // disabled reject, cross-tenant reject) is covered end-to-end by
    // tests/login_flow.rs. The unit tests here pin the helper that
    // formats the 403 response so its KNJK header shape cannot drift.

    #[tokio::test]
    async fn auth_error_response_for_admin_forbidden_emits_knjk_4033() {
        let resp = auth_error_response(AuthErrorCode::AdminForbidden);
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let code = resp
            .headers()
            .get("x-knjk-error-code")
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        assert_eq!(code.as_deref(), Some("KNJK-4033"));
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"], "Admin access required");
        assert_eq!(v["success"], false);
    }
}
