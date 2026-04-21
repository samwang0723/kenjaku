//! Integration tests for `POST /api/v1/auth/login` + the admin router.
//!
//! Pattern mirrors `tests/auth_flow.rs`: we don't spin up the real
//! `AppState` (which needs SearchService + Qdrant + Redis + Postgres).
//! Instead we build a minimal `LoginFlowState` carrying just the
//! moving parts — an in-memory `UsersStore`, a real `JwtMinter` +
//! `JwtValidator` against a seeded test keypair, and a
//! `TenantsCache` of known tenants.
//!
//! The parallel handlers below compile against this in-memory state
//! but implement the **same decision tree** as the production
//! handlers in `handlers::auth` and `handlers::admin_users`. Signature
//! anchors at the bottom of the file prevent the production fn shapes
//! from drifting.

#![allow(clippy::too_many_arguments)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::{Body, to_bytes};
use axum::extract::{Path, State};
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use kenjaku_api::dto::auth::{AdminUserDto, LoginRequest, LoginResponse};
use kenjaku_core::config::{JwtAlgorithm, JwtConfig};
use kenjaku_core::error::AuthErrorCode;
use kenjaku_core::types::tenant::{PlanTier, PrincipalId, TenantContext, TenantId};
use kenjaku_infra::auth::{JwtMinter, JwtValidator};
use kenjaku_infra::postgres::{Role, TenantRow, TenantsCache, UsersRepository};

// -------- Keypair fixture (seeded so failures reproduce) --------

struct TestKeypair {
    private_pem: String,
    public_pem: String,
}

fn keypair() -> &'static TestKeypair {
    static KP: OnceLock<TestKeypair> = OnceLock::new();
    KP.get_or_init(|| {
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

const TEST_ISSUER: &str = "kenjaku-it";
const TEST_AUDIENCE: &str = "kenjaku-api";

fn jwt_cfg() -> JwtConfig {
    JwtConfig {
        issuer: TEST_ISSUER.into(),
        audience: TEST_AUDIENCE.into(),
        public_key_path: "<t>".into(),
        private_key_path: "<t>".into(),
        ttl_seconds: 3600,
        algorithm: JwtAlgorithm::RS256,
        clock_skew_secs: 5,
    }
}

fn make_minter() -> Arc<JwtMinter> {
    Arc::new(JwtMinter::new(&jwt_cfg(), keypair().private_pem.as_bytes()).expect("minter"))
}

fn make_validator() -> Arc<JwtValidator> {
    Arc::new(JwtValidator::new(&jwt_cfg(), keypair().public_pem.as_bytes()).expect("validator"))
}

// -------- In-memory users store (stand-in for live Postgres) -----

#[derive(Clone)]
struct UserEntry {
    id: Uuid,
    tenant_id: String,
    email: String,
    password_hash: String,
    role: Role,
    enabled: bool,
    created_at: DateTime<Utc>,
    last_login_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Default)]
struct UsersStore {
    rows: Arc<Mutex<Vec<UserEntry>>>,
}

impl UsersStore {
    fn insert(&self, row: UserEntry) {
        self.rows.lock().unwrap().push(row);
    }
    fn find_by_email(&self, email: &str) -> Option<UserEntry> {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.email.eq_ignore_ascii_case(email))
            .cloned()
    }
    fn find_by_id(&self, id: Uuid, tenant_id: &str) -> Option<UserEntry> {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.id == id && r.tenant_id == tenant_id)
            .cloned()
    }
    fn list_by_tenant(&self, tenant_id: &str) -> Vec<UserEntry> {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.tenant_id == tenant_id)
            .cloned()
            .collect()
    }
    #[allow(dead_code)] // used by future admin-update-role test
    fn update_role(&self, id: Uuid, tenant_id: &str, role: Role) -> bool {
        let mut guard = self.rows.lock().unwrap();
        if let Some(row) = guard
            .iter_mut()
            .find(|r| r.id == id && r.tenant_id == tenant_id)
        {
            row.role = role;
            return true;
        }
        false
    }
    fn set_enabled(&self, id: Uuid, tenant_id: &str, enabled: bool) -> bool {
        let mut guard = self.rows.lock().unwrap();
        if let Some(row) = guard
            .iter_mut()
            .find(|r| r.id == id && r.tenant_id == tenant_id)
        {
            row.enabled = enabled;
            return true;
        }
        false
    }
    fn touch_last_login(&self, id: Uuid) {
        let mut guard = self.rows.lock().unwrap();
        if let Some(row) = guard.iter_mut().find(|r| r.id == id) {
            row.last_login_at = Some(Utc::now());
        }
    }
    fn create(
        &self,
        tenant_id: &str,
        email: &str,
        password_hash: &str,
        role: Role,
    ) -> Result<UserEntry, String> {
        if self
            .rows
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.email.eq_ignore_ascii_case(email))
        {
            return Err(format!("email {email:?} already exists"));
        }
        let row = UserEntry {
            id: Uuid::new_v4(),
            tenant_id: tenant_id.to_string(),
            email: email.to_string(),
            password_hash: password_hash.to_string(),
            role,
            enabled: true,
            created_at: Utc::now(),
            last_login_at: None,
        };
        self.rows.lock().unwrap().push(row.clone());
        Ok(row)
    }
}

fn entry_to_dto(row: &UserEntry) -> AdminUserDto {
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

// -------- Shared in-proc state --------

#[derive(Clone)]
struct LoginFlowState {
    users: UsersStore,
    tenants: Arc<TenantsCache>,
    minter: Arc<JwtMinter>,
    validator: Arc<JwtValidator>,
}

fn tenants_cache(rows: &[(&str, PlanTier)]) -> Arc<TenantsCache> {
    let mut m = HashMap::new();
    for (id, plan) in rows {
        let tid = TenantId::new(*id).unwrap();
        m.insert(
            tid.clone(),
            Arc::new(TenantRow {
                id: tid,
                name: format!("{id} Corp"),
                plan_tier: *plan,
                config_overrides: json!({}),
            }),
        );
    }
    Arc::new(TenantsCache::from_map(m))
}

fn seed_user(store: &UsersStore, tenant: &str, email: &str, password: &str, role: Role) -> Uuid {
    let hash = UsersRepository::hash_password(password).unwrap();
    let id = Uuid::new_v4();
    store.insert(UserEntry {
        id,
        tenant_id: tenant.to_string(),
        email: email.to_string(),
        password_hash: hash,
        role,
        enabled: true,
        created_at: Utc::now(),
        last_login_at: None,
    });
    id
}

// -------- Parallel login handler (same decision tree as production) --

async fn login_handler(
    State(state): State<Arc<LoginFlowState>>,
    Json(body): Json<LoginRequest>,
) -> Response {
    use argon2::Argon2;
    use argon2::password_hash::{PasswordHash, PasswordVerifier};

    let invalid = || -> Response {
        let mut resp = (
            StatusCode::UNAUTHORIZED,
            Json(json!({"success": false, "error": "Invalid email or password"})),
        )
            .into_response();
        resp.headers_mut().insert(
            "x-knjk-error-code",
            AuthErrorCode::InvalidCredentials.code().parse().unwrap(),
        );
        resp
    };

    let Some(user) = state.users.find_by_email(&body.email) else {
        return invalid();
    };
    if !user.enabled {
        return invalid();
    }
    let Ok(parsed) = PasswordHash::new(&user.password_hash) else {
        return invalid();
    };
    if Argon2::default()
        .verify_password(body.password.as_bytes(), &parsed)
        .is_err()
    {
        return invalid();
    }

    let Ok(tid) = TenantId::new(user.tenant_id.clone()) else {
        return invalid();
    };
    let Some(row) = state.tenants.get(&tid) else {
        return invalid();
    };

    let pid = PrincipalId::new(user.id.to_string()).unwrap();
    let (token, exp) = state
        .minter
        .mint(&tid, Some(&pid), Some(row.plan_tier), None)
        .expect("mint");

    state.users.touch_last_login(user.id);

    let resp = LoginResponse {
        token,
        expires_at: exp.to_rfc3339(),
        tenant_id: user.tenant_id,
        email: user.email,
        role: user.role.as_str().to_string(),
    };
    (StatusCode::OK, Json(json!({"success": true, "data": resp}))).into_response()
}

// -------- Parallel admin router --------

async fn auth_mw(
    State(state): State<Arc<LoginFlowState>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let validator = state.validator.clone();
    let bearer = req
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(bearer) = bearer else {
        return (StatusCode::UNAUTHORIZED, "unauth").into_response();
    };
    let claims = match validator.validate(bearer) {
        Ok(c) => c,
        Err(_) => return (StatusCode::UNAUTHORIZED, "unauth").into_response(),
    };
    let Some(row) = state.tenants.get(&claims.tenant_id) else {
        return (StatusCode::FORBIDDEN, "tenant").into_response();
    };
    let tctx = TenantContext {
        tenant_id: claims.tenant_id,
        principal_id: claims.principal_id,
        plan_tier: row.plan_tier,
    };
    req.extensions_mut().insert(tctx);
    next.run(req).await
}

async fn require_admin_mw(
    State(state): State<Arc<LoginFlowState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let Some(tctx) = req.extensions().get::<TenantContext>().cloned() else {
        return forbidden();
    };
    let Some(pid) = tctx.principal_id.as_ref() else {
        return forbidden();
    };
    let Ok(uuid) = Uuid::parse_str(pid.as_str()) else {
        return forbidden();
    };
    let Some(user) = state.users.find_by_id(uuid, tctx.tenant_id.as_str()) else {
        return forbidden();
    };
    if !user.enabled || !matches!(user.role, Role::Admin) {
        return forbidden();
    }
    next.run(req).await
}

fn forbidden() -> Response {
    let mut resp = (
        StatusCode::FORBIDDEN,
        Json(json!({"success": false, "error": "Admin access required"})),
    )
        .into_response();
    resp.headers_mut().insert(
        "x-knjk-error-code",
        AuthErrorCode::AdminForbidden.code().parse().unwrap(),
    );
    resp
}

async fn list_users_handler(
    State(state): State<Arc<LoginFlowState>>,
    req: Request<Body>,
) -> Response {
    let tctx = req.extensions().get::<TenantContext>().unwrap().clone();
    let items: Vec<AdminUserDto> = state
        .users
        .list_by_tenant(tctx.tenant_id.as_str())
        .iter()
        .map(entry_to_dto)
        .collect();
    (
        StatusCode::OK,
        Json(json!({"success": true, "data": items})),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
struct CreateBody {
    email: String,
    role: String,
    password: String,
}

async fn create_user_handler(
    State(state): State<Arc<LoginFlowState>>,
    req: Request<Body>,
) -> Response {
    let tctx = req.extensions().get::<TenantContext>().unwrap().clone();
    // Extract body manually (we already consumed extensions).
    let body_bytes = to_bytes(req.into_body(), 64 * 1024).await.unwrap();
    let body: CreateBody = match serde_json::from_slice(&body_bytes) {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad body").into_response(),
    };
    const MIN_PW: usize = 12;
    if body.password.len() < MIN_PW {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "password too short"})),
        )
            .into_response();
    }
    let role = match body.role.as_str() {
        "admin" => Role::Admin,
        "member" => Role::Member,
        _ => return (StatusCode::BAD_REQUEST, Json(json!({"error": "bad role"}))).into_response(),
    };
    let hash = UsersRepository::hash_password(&body.password).unwrap();
    match state
        .users
        .create(tctx.tenant_id.as_str(), &body.email, &hash, role)
    {
        Ok(row) => (
            StatusCode::CREATED,
            Json(json!({"success": true, "data": entry_to_dto(&row)})),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    }
}

fn build_router(state: Arc<LoginFlowState>) -> Router {
    let admin = Router::new()
        .route(
            "/admin/users",
            get(list_users_handler).post(create_user_handler),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_admin_mw,
        ))
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth_mw));

    let auth = Router::new().route("/auth/login", post(login_handler));

    Router::new()
        .nest("/api/v1", admin.merge(auth))
        .with_state(state)
}

async fn body_json(resp: Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(json!(null))
}

// ============================================================
// Tests
// ============================================================

fn make_state() -> Arc<LoginFlowState> {
    let users = UsersStore::default();
    // Seed the "public" admin.
    seed_user(&users, "public", "admin@public.com", "admin", Role::Admin);
    Arc::new(LoginFlowState {
        users,
        tenants: tenants_cache(&[("public", PlanTier::Enterprise), ("acme", PlanTier::Pro)]),
        minter: make_minter(),
        validator: make_validator(),
    })
}

#[tokio::test]
async fn login_happy_path_returns_200_with_token_and_role() {
    let state = make_state();
    let app = build_router(state.clone());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"email":"admin@public.com","password":"admin"}"#,
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    assert_eq!(body["success"], true);
    let data = &body["data"];
    assert_eq!(data["tenant_id"], "public");
    assert_eq!(data["email"], "admin@public.com");
    assert_eq!(data["role"], "admin");
    let token = data["token"].as_str().unwrap();
    assert!(!token.is_empty());

    // Round-trip: the minted token validates under the same validator.
    let claims = state.validator.validate(token).unwrap();
    assert_eq!(claims.tenant_id.as_str(), "public");
    assert_eq!(claims.plan_tier, Some(PlanTier::Enterprise));
}

#[tokio::test]
async fn login_wrong_password_returns_401_invalid_credentials() {
    let app = build_router(make_state());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"email":"admin@public.com","password":"WRONG"}"#,
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 401);
    let code = resp
        .headers()
        .get("x-knjk-error-code")
        .and_then(|h| h.to_str().ok());
    assert_eq!(code, Some("KNJK-4011"));
    let body = body_json(resp).await;
    assert_eq!(body["error"], "Invalid email or password");
}

#[tokio::test]
async fn login_unknown_email_returns_401_invalid_credentials() {
    let app = build_router(make_state());
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"email":"nobody@nowhere.com","password":"whatever"}"#,
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // SECURITY: same 401 + same code as wrong-password — no
    // enumeration leak.
    assert_eq!(resp.status(), 401);
    let code = resp
        .headers()
        .get("x-knjk-error-code")
        .and_then(|h| h.to_str().ok());
    assert_eq!(code, Some("KNJK-4011"));
}

#[tokio::test]
async fn login_disabled_user_returns_401_same_as_wrong_password() {
    let state = make_state();
    // Disable the seeded admin.
    let id = state.users.find_by_email("admin@public.com").unwrap().id;
    state.users.set_enabled(id, "public", false);

    let app = build_router(state);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"email":"admin@public.com","password":"admin"}"#,
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 401);
    assert_eq!(
        resp.headers()
            .get("x-knjk-error-code")
            .and_then(|h| h.to_str().ok()),
        Some("KNJK-4011")
    );
}

#[tokio::test]
async fn login_user_with_missing_tenant_returns_401_no_enumeration() {
    let state = make_state();
    // User row whose tenant_id isn't in the cache.
    seed_user(
        &state.users,
        "ghost-tenant",
        "orphan@ghost-tenant.com",
        "password12ch",
        Role::Admin,
    );
    let app = build_router(state);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"email":"orphan@ghost-tenant.com","password":"password12ch"}"#,
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 401);
    assert_eq!(
        resp.headers()
            .get("x-knjk-error-code")
            .and_then(|h| h.to_str().ok()),
        Some("KNJK-4011")
    );
}

#[tokio::test]
async fn admin_list_users_as_admin_returns_200_with_tenant_only_rows() {
    let state = make_state();
    // Seed an acme admin + a public member so we can assert
    // cross-tenant isolation.
    let acme_admin_id = seed_user(
        &state.users,
        "acme",
        "a@acme.com",
        "anypassword12",
        Role::Admin,
    );
    seed_user(
        &state.users,
        "public",
        "m@public.com",
        "anypassword12",
        Role::Member,
    );

    // Mint a JWT for the acme admin.
    let tid = TenantId::new("acme").unwrap();
    let pid = PrincipalId::new(acme_admin_id.to_string()).unwrap();
    let (token, _) = state
        .minter
        .mint(&tid, Some(&pid), Some(PlanTier::Pro), None)
        .unwrap();

    let app = build_router(state);
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/admin/users")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    let items = body["data"].as_array().unwrap();
    // acme admin only — the public admin + public member are invisible.
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["email"], "a@acme.com");
    assert_eq!(items[0]["tenant_id"], "acme");
}

#[tokio::test]
async fn admin_list_users_as_member_returns_403_admin_forbidden() {
    let state = make_state();
    let id = seed_user(
        &state.users,
        "public",
        "just-a-member@public.com",
        "anypassword12",
        Role::Member,
    );
    let tid = TenantId::new("public").unwrap();
    let pid = PrincipalId::new(id.to_string()).unwrap();
    let (token, _) = state
        .minter
        .mint(&tid, Some(&pid), Some(PlanTier::Enterprise), None)
        .unwrap();

    let app = build_router(state);
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/admin/users")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 403);
    let code = resp
        .headers()
        .get("x-knjk-error-code")
        .and_then(|h| h.to_str().ok());
    assert_eq!(code, Some("KNJK-4033"));
}

#[tokio::test]
async fn admin_list_users_cross_tenant_isolation() {
    // Tenant A admin cannot see tenant B rows — a classic
    // privilege-escalation regression guard.
    let state = make_state();
    seed_user(
        &state.users,
        "acme",
        "only-acme@acme.com",
        "anypassword12",
        Role::Member,
    );

    // Mint a JWT for the PUBLIC admin (seeded by make_state).
    let admin_id = state.users.find_by_email("admin@public.com").unwrap().id;
    let tid = TenantId::new("public").unwrap();
    let pid = PrincipalId::new(admin_id.to_string()).unwrap();
    let (token, _) = state
        .minter
        .mint(&tid, Some(&pid), Some(PlanTier::Enterprise), None)
        .unwrap();

    let app = build_router(state);
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/admin/users")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    let items = body["data"].as_array().unwrap();
    // One row only: admin@public.com. `only-acme@acme.com` is
    // invisible to the public admin.
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["tenant_id"], "public");
    assert_eq!(items[0]["email"], "admin@public.com");
}

#[tokio::test]
async fn admin_create_user_rejects_short_password_and_accepts_min_len() {
    let state = make_state();
    let admin_id = state.users.find_by_email("admin@public.com").unwrap().id;
    let tid = TenantId::new("public").unwrap();
    let pid = PrincipalId::new(admin_id.to_string()).unwrap();
    let (token, _) = state
        .minter
        .mint(&tid, Some(&pid), Some(PlanTier::Enterprise), None)
        .unwrap();

    let app = build_router(state.clone());

    // 11-char password fails.
    let short = r#"{"email":"new@public.com","role":"member","password":"Abcdef12345"}"#;
    assert_eq!("Abcdef12345".len(), 11, "fixture tracks MIN_PW=12 boundary");
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/admin/users")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(short))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 400);

    // 12-char succeeds.
    let ok = r#"{"email":"new@public.com","role":"member","password":"Abcdef123456"}"#;
    let app = build_router(state);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/admin/users")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(ok))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 201);
}

// -------- Production-handler signature anchors --------
//
// Linking the production handlers into the integration binary fails
// the IT compile whenever the public signatures drift — protects us
// against a refactor quietly breaking the wire contract the Vec below
// depends on.

type AnchorFut = std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send>>;

type LoginFn =
    fn(State<Arc<kenjaku_api::AppState>>, Json<kenjaku_api::dto::auth::LoginRequest>) -> AnchorFut;

type RequireAdminFn = fn(State<Arc<kenjaku_api::AppState>>, Request<Body>, Next) -> AnchorFut;

#[allow(dead_code)]
fn _login_signature_anchor() -> LoginFn {
    |s, b| Box::pin(kenjaku_api::handlers::auth::login(s, b))
}

#[allow(dead_code)]
fn _require_admin_signature_anchor() -> RequireAdminFn {
    |s, r, n| {
        Box::pin(kenjaku_api::middleware::require_admin::require_admin(
            s, r, n,
        ))
    }
}

// Unused-path suppression: `Path<Uuid>` referenced only transitively
// by the prod handlers; wire it here so `use` stays honest.
#[allow(dead_code)]
fn _uses_path() -> Path<Uuid> {
    Path(Uuid::nil())
}
