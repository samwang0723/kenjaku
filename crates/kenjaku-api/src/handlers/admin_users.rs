//! Admin user-management handlers — `GET/POST/PATCH/DELETE
//! /api/v1/admin/users` and `POST /api/v1/admin/users/:id/reset-password`.
//!
//! Every handler is scoped to `tctx.tenant_id` — a tenant-A admin
//! cannot touch tenant-B rows. Enforcement is via the tenant-scoped
//! `UsersRepository` methods, which reject on `(id, tenant_id)`
//! mismatch.
//!
//! Mounted under the `require_admin` middleware, so reaching any of
//! these handlers implies the caller's user row is `enabled=true` and
//! `role=admin` in their own tenant.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Json, response::Response};
use tracing::{error, instrument};
use uuid::Uuid;

use kenjaku_core::error::Error;
use kenjaku_infra::postgres::{Role, UserRow, UsersRepository};

use crate::AppState;
use crate::dto::auth::{AdminUserDto, CreateUserRequest, ResetPasswordRequest, UpdateUserRequest};
use crate::dto::response::ApiResponse;
use crate::extractors::TenantCtx;

/// Minimum password length enforced on every `/admin/*` password-
/// accepting endpoint. Mirrored by the frontend for UX.
///
/// The CLI path deliberately bypasses this check so operators can
/// seed deterministic default credentials (`admin@{tenant}.com` /
/// `admin`) without losing the web-side safety net.
pub const MIN_ADMIN_PASSWORD_LEN: usize = 12;

/// GET /api/v1/admin/users — list users in the caller's tenant.
#[instrument(skip(state))]
pub async fn list_users(
    TenantCtx(tctx): TenantCtx,
    State(state): State<Arc<AppState>>,
) -> Response {
    match state
        .users_repo
        .list_by_tenant(tctx.tenant_id.as_str())
        .await
    {
        Ok(rows) => {
            let items: Vec<AdminUserDto> = rows.iter().map(to_dto).collect();
            (StatusCode::OK, Json(ApiResponse::ok(items))).into_response()
        }
        Err(e) => {
            error!(error = %e, "list_users failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<Vec<AdminUserDto>>::err(
                    e.user_message().to_string(),
                )),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/admin/users — create a user in the caller's tenant.
#[instrument(skip(state, body))]
pub async fn create_user(
    TenantCtx(tctx): TenantCtx,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateUserRequest>,
) -> Response {
    if body.email.is_empty() || body.email.len() > 320 {
        return bad_request("invalid email");
    }
    if body.password.len() < MIN_ADMIN_PASSWORD_LEN {
        return bad_request(&format!(
            "password must be at least {MIN_ADMIN_PASSWORD_LEN} characters"
        ));
    }
    let role = match body.role.as_str() {
        "admin" => Role::Admin,
        "member" => Role::Member,
        other => return bad_request(&format!("invalid role {other:?}")),
    };

    let hash = match UsersRepository::hash_password(&body.password) {
        Ok(h) => h,
        Err(e) => {
            error!(error = %e, "create_user: hash_password failed");
            return internal_error("hash failed");
        }
    };

    match state
        .users_repo
        .create(tctx.tenant_id.as_str(), &body.email, &hash, role)
        .await
    {
        Ok(row) => (StatusCode::CREATED, Json(ApiResponse::ok(to_dto(&row)))).into_response(),
        Err(e @ Error::Validation(_)) => bad_request(e.user_message()),
        Err(e) => {
            error!(error = %e, "create_user failed");
            internal_error(e.user_message())
        }
    }
}

/// PATCH /api/v1/admin/users/:id — update role / enabled in own tenant.
#[instrument(skip(state, body))]
pub async fn update_user(
    TenantCtx(tctx): TenantCtx,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUserRequest>,
) -> Response {
    if body.role.is_none() && body.enabled.is_none() {
        return bad_request("no fields to update");
    }

    if let Some(role_str) = body.role.as_deref() {
        let role = match role_str {
            "admin" => Role::Admin,
            "member" => Role::Member,
            other => return bad_request(&format!("invalid role {other:?}")),
        };
        match state
            .users_repo
            .update_role(id, tctx.tenant_id.as_str(), role)
            .await
        {
            Ok(true) => {}
            Ok(false) => return not_found("user not found in tenant"),
            Err(e) => {
                error!(error = %e, "update_user: update_role failed");
                return internal_error(e.user_message());
            }
        }
    }

    if let Some(enabled) = body.enabled {
        match state
            .users_repo
            .set_enabled(id, tctx.tenant_id.as_str(), enabled)
            .await
        {
            Ok(true) => {}
            Ok(false) => return not_found("user not found in tenant"),
            Err(e) => {
                error!(error = %e, "update_user: set_enabled failed");
                return internal_error(e.user_message());
            }
        }
    }

    match state
        .users_repo
        .find_by_id(id, tctx.tenant_id.as_str())
        .await
    {
        Ok(Some(row)) => (StatusCode::OK, Json(ApiResponse::ok(to_dto(&row)))).into_response(),
        Ok(None) => not_found("user not found after update"),
        Err(e) => {
            error!(error = %e, "update_user: post-update find failed");
            internal_error(e.user_message())
        }
    }
}

/// POST /api/v1/admin/users/:id/reset-password — force a new password.
#[instrument(skip(state, body))]
pub async fn reset_password(
    TenantCtx(tctx): TenantCtx,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<ResetPasswordRequest>,
) -> Response {
    if body.password.len() < MIN_ADMIN_PASSWORD_LEN {
        return bad_request(&format!(
            "password must be at least {MIN_ADMIN_PASSWORD_LEN} characters"
        ));
    }
    let hash = match UsersRepository::hash_password(&body.password) {
        Ok(h) => h,
        Err(e) => {
            error!(error = %e, "reset_password: hash failed");
            return internal_error("hash failed");
        }
    };
    match state
        .users_repo
        .update_password_hash(id, tctx.tenant_id.as_str(), &hash)
        .await
    {
        Ok(true) => (StatusCode::NO_CONTENT, ()).into_response(),
        Ok(false) => not_found("user not found in tenant"),
        Err(e) => {
            error!(error = %e, "reset_password failed");
            internal_error(e.user_message())
        }
    }
}

/// DELETE /api/v1/admin/users/:id — soft delete (sets enabled=false).
#[instrument(skip(state))]
pub async fn delete_user(
    TenantCtx(tctx): TenantCtx,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Response {
    match state
        .users_repo
        .set_enabled(id, tctx.tenant_id.as_str(), false)
        .await
    {
        Ok(true) => (StatusCode::NO_CONTENT, ()).into_response(),
        Ok(false) => not_found("user not found in tenant"),
        Err(e) => {
            error!(error = %e, "delete_user failed");
            internal_error(e.user_message())
        }
    }
}

/// Convert an infra `UserRow` into the wire DTO. Omits `password_hash`
/// by construction.
fn to_dto(row: &UserRow) -> AdminUserDto {
    AdminUserDto {
        id: row.id.to_string(),
        tenant_id: row.tenant_id.clone(),
        email: row.email.clone(),
        role: row.role.as_str().to_string(),
        enabled: row.enabled,
        created_at: row.created_at.to_rfc3339(),
        last_login_at: row.last_login_at.map(|t| t.to_rfc3339()),
    }
}

fn bad_request(msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::<()>::err(msg.to_string())),
    )
        .into_response()
}

fn not_found(msg: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ApiResponse::<()>::err(msg.to_string())),
    )
        .into_response()
}

fn internal_error(msg: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResponse::<()>::err(msg.to_string())),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use chrono::Utc;

    // The full admin handler request → response path is covered end-
    // to-end by tests/login_flow.rs. These unit tests pin the pure
    // helpers + DTO conversions that don't need HTTP wiring.

    #[tokio::test]
    async fn bad_request_returns_400_with_message() {
        let resp = bad_request("nope");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error"], "nope");
    }

    #[tokio::test]
    async fn not_found_returns_404_with_message() {
        let resp = not_found("missing");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"], "missing");
    }

    #[tokio::test]
    async fn internal_error_returns_500_with_generic_message() {
        let resp = internal_error("boom");
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"], "boom");
    }

    #[test]
    fn to_dto_serialises_all_fields_and_omits_last_login_when_none() {
        let row = UserRow {
            id: uuid::Uuid::nil(),
            tenant_id: "public".into(),
            email: "a@public.com".into(),
            password_hash: "$argon2id$...$redacted".into(),
            role: Role::Admin,
            enabled: true,
            created_at: Utc::now(),
            last_login_at: None,
        };
        let dto = to_dto(&row);
        assert_eq!(dto.id, "00000000-0000-0000-0000-000000000000");
        assert_eq!(dto.tenant_id, "public");
        assert_eq!(dto.email, "a@public.com");
        assert_eq!(dto.role, "admin");
        assert!(dto.enabled);
        assert!(dto.last_login_at.is_none());
    }

    #[test]
    fn to_dto_preserves_last_login_when_present() {
        let now = Utc::now();
        let row = UserRow {
            id: uuid::Uuid::nil(),
            tenant_id: "acme".into(),
            email: "x@acme.com".into(),
            password_hash: "".into(),
            role: Role::Member,
            enabled: false,
            created_at: now,
            last_login_at: Some(now),
        };
        let dto = to_dto(&row);
        assert_eq!(dto.role, "member");
        assert!(!dto.enabled);
        assert!(dto.last_login_at.is_some());
    }

    #[test]
    fn min_admin_password_len_is_twelve() {
        // Boundary is baked into both the admin handlers and the
        // smoke-test expectations; locking with a unit test keeps them
        // in sync.
        assert_eq!(MIN_ADMIN_PASSWORD_LEN, 12);
    }
}
