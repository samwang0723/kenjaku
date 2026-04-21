//! Auth + admin DTOs for the `auth-login-rbac` feature.
//!
//! Wire shapes for:
//!  * `POST /api/v1/auth/login`
//!  * `GET/POST/PATCH/DELETE /api/v1/admin/users`
//!  * `POST /api/v1/admin/users/:id/reset-password`
//!
//! Kept on a single module because the shapes are tightly coupled —
//! the login response returns the same `role` string that `AdminUserDto`
//! surfaces, and splitting would duplicate the enum serialisation rules.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// `POST /api/v1/auth/login` body.
///
/// Both fields are required. Max lengths mirror sensible web-form
/// limits — anything above these is almost certainly malicious /
/// misconfigured. Further argon2 verification is timing-safe regardless,
/// so these bounds are UX + DoS guards, not security primitives.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// `POST /api/v1/auth/login` success payload. The client stores
/// `token` + `expires_at` in localStorage and includes
/// `Authorization: Bearer <token>` on every subsequent API call.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LoginResponse {
    pub token: String,
    /// RFC3339 timestamp matching the JWT `exp` claim.
    pub expires_at: String,
    pub tenant_id: String,
    pub email: String,
    /// `"admin"` or `"member"` — mirrors `kenjaku_infra::postgres::Role`.
    pub role: String,
}

/// `POST /api/v1/admin/users` body — create a user in the caller's
/// own tenant.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct CreateUserRequest {
    pub email: String,
    /// `"admin"` or `"member"`.
    pub role: String,
    /// Initial password. The handler enforces the
    /// `MIN_ADMIN_PASSWORD_LEN` minimum server-side; clients should
    /// mirror for UX.
    pub password: String,
}

/// `PATCH /api/v1/admin/users/:id` body. Both fields optional —
/// callers may flip one or the other. When both are `None` the
/// handler returns a 400.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
pub struct UpdateUserRequest {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// `POST /api/v1/admin/users/:id/reset-password` body.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ResetPasswordRequest {
    pub password: String,
}

/// Public user summary — the admin UI team-list card renders one of
/// these per row. `password_hash` is deliberately absent: the server
/// never discloses it.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AdminUserDto {
    pub id: String,
    pub tenant_id: String,
    pub email: String,
    pub role: String,
    pub enabled: bool,
    /// RFC3339 timestamps.
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_login_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_request_parses_minimal_body() {
        let dto: LoginRequest =
            serde_json::from_str(r#"{"email":"admin@public.com","password":"admin"}"#).unwrap();
        assert_eq!(dto.email, "admin@public.com");
        assert_eq!(dto.password, "admin");
    }

    #[test]
    fn login_response_serialises_all_fields() {
        let resp = LoginResponse {
            token: "header.payload.sig".into(),
            expires_at: "2026-04-22T12:00:00Z".into(),
            tenant_id: "public".into(),
            email: "admin@public.com".into(),
            role: "admin".into(),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["token"], "header.payload.sig");
        assert_eq!(v["role"], "admin");
        assert_eq!(v["tenant_id"], "public");
    }

    #[test]
    fn admin_user_dto_omits_null_last_login() {
        let row = AdminUserDto {
            id: "00000000-0000-0000-0000-000000000001".into(),
            tenant_id: "acme".into(),
            email: "alice@acme.com".into(),
            role: "member".into(),
            enabled: true,
            created_at: "2026-04-21T00:00:00Z".into(),
            last_login_at: None,
        };
        let v = serde_json::to_value(&row).unwrap();
        assert!(v.get("last_login_at").is_none());
    }

    #[test]
    fn update_user_request_parses_partial_body() {
        // Either field can be set alone.
        let a: UpdateUserRequest = serde_json::from_str(r#"{"role":"admin"}"#).unwrap();
        assert_eq!(a.role.as_deref(), Some("admin"));
        assert_eq!(a.enabled, None);
        let b: UpdateUserRequest = serde_json::from_str(r#"{"enabled":false}"#).unwrap();
        assert_eq!(b.enabled, Some(false));
        assert!(b.role.is_none());
        let c: UpdateUserRequest = serde_json::from_str(r#"{}"#).unwrap();
        assert!(c.role.is_none() && c.enabled.is_none());
    }
}
