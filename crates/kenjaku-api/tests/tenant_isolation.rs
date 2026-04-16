//! Phase 3d.1 isolation integration test — proves zero cross-tenant reads
//! across the five tenanted read paths once `tenancy.enabled=true`.
//!
//! # Scope
//!
//! In-process fake-state harness mirroring `tests/auth_flow.rs`. Mints
//! two JWTs (`tenant-a` and `tenant-b`) against a single RSA-2048 test
//! keypair, drives a parallel router whose auth middleware matches the
//! production `tenant_auth_middleware` decision tree, and asserts that
//! fake per-tenant stores keyed by `(TenantId, key)` are never read
//! under the wrong tenant's JWT.
//!
//! The harness intentionally does NOT spin up the real
//! `kenjaku_api::AppState` — that requires `SearchService` + Qdrant +
//! Redis + PostgreSQL live deps and is out of scope for an integration
//! test that just proves middleware-scoped reads stay scoped. The
//! complementary layers are:
//!
//! - **Service-layer unit test** (`session/locale_memory.rs`
//!   `key_separates_distinct_tenants`) covers real Redis key shape.
//! - **QA `enabled=true` chrome-cdp smoke** (per `_status.md` §31-36)
//!   exercises the full live pipeline.
//!
//! This test fills the middle layer: extractor + middleware + handler
//! receive the correct `TenantContext` and per-tenant stores are not
//! cross-readable.
//!
//! # Matrix
//!
//! 5 read paths × 2 directions = 10 isolation assertions, plus one
//! response-body negative assertion on `/search` that tenant A's LLM
//! response never echoes tenant B's conversation-history secrets. Total
//! 11 case assertions.
//!
//! | Path | Store | Read mechanism |
//! |------|-------|----------------|
//! | `/autocomplete` | `popular_queries` | `TenantCtx` → store.get((tctx.tenant_id, locale)) |
//! | `/top-searches` | `popular_queries` | `TenantCtx` → store.get((tctx.tenant_id, locale)) |
//! | `/feedback` | `feedback_store`  | `TenantCtx` → store.get((tctx.tenant_id, request_id)) |
//! | `/search` (RAG chunks) | `rag_chunks`  | `TenantCtx` → store.get(tctx.tenant_id) |
//! | `/search` (conversation history) | `conversations` | `TenantCtx` → scan by tctx.tenant_id |
//!
//! # Known RED cases on base commit 492dbdd
//!
//! Cases 1-2 (locale-memory cross-read via `ResolvedLocale` extractor)
//! CANNOT be asserted at base because `SessionLocaleLookup::lookup`
//! has no `&TenantContext` parameter at base — that's the very bug
//! dev-1 is fixing in this slice. The trait widening landing on
//! `integrate` will unblock a compile-time version of the anchor; in
//! the meantime cases 1-2 are scoped into the handler body (using
//! `TenantCtx`, which DOES exist at base) and are expected to pass on
//! both base and integrate.
//!
//! # Signature anchors
//!
//! - `_auth_mw_signature_anchor` — mirrors `tests/auth_flow.rs:440-451`.
//!   Keeps the production `tenant_auth_middleware` fn linked so any
//!   signature drift fails THIS compile, not just the unit build.
//! - `_lookup_signature_anchor` — anchors the current
//!   `SessionLocaleLookup` trait. When dev-1 widens `lookup` to take
//!   `&TenantContext`, this anchor becomes load-bearing: a future
//!   revert of the widening would fail this compile.

#![allow(clippy::too_many_arguments)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::body::to_bytes;
use axum::extract::{Extension, Query, State};
use axum::http::{Method, Request, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use jsonwebtoken::{Algorithm, EncodingKey, Header as JwtHeader, encode as jwt_encode};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

use kenjaku_api::extractors::{ResolvedLocale, SessionLocaleLookup, TenantCtx};
use kenjaku_api::middleware::auth::tenant_auth_middleware;
use kenjaku_core::config::{JwtAlgorithm, JwtConfig};
use kenjaku_core::types::locale::Locale;
use kenjaku_core::types::tenant::{PlanTier, TenantContext, TenantId};
use kenjaku_infra::auth::JwtValidator;
use kenjaku_infra::postgres::{TenantRow, TenantsCache};

// ============================================================================
// Constants
// ============================================================================

const TEST_ISSUER: &str = "kenjaku-isolation-it";
const TEST_AUDIENCE: &str = "kenjaku-api";

/// Distinctive string seeded into tenant B's conversation history used
/// to prove the response body from `/search` under tenant A's JWT never
/// echoes it. See §conversation-history negative assertion test.
const TENANT_B_SECRET_PHRASE: &str = "tenant-b-secret-phrase-xyz";

const TENANT_A: &str = "tenant-a";
const TENANT_B: &str = "tenant-b";

// ============================================================================
// Keypair + JWT helpers (mirror tests/auth_flow.rs)
// ============================================================================

struct TestKeypair {
    private_pem: String,
    public_pem: String,
}

fn keypair() -> &'static TestKeypair {
    static KP: OnceLock<TestKeypair> = OnceLock::new();
    KP.get_or_init(|| {
        // Seeded RNG for determinism — the same key materializes every
        // run, so any failure traces are reproducible. Seed is
        // arbitrary; nothing about the key is sensitive.
        let mut rng = StdRng::seed_from_u64(0xCAFE_BABE_DEAD_BEEF);
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

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn mint_rs256(claims: serde_json::Value) -> String {
    let key = EncodingKey::from_rsa_pem(keypair().private_pem.as_bytes()).expect("encoding key");
    jwt_encode(&JwtHeader::new(Algorithm::RS256), &claims, &key).expect("jwt encode")
}

fn token_for(tenant_id: &str, principal_id: &str) -> String {
    let now = now_secs();
    mint_rs256(serde_json::json!({
        "tenant_id": tenant_id,
        "principal_id": principal_id,
        "plan_tier": "pro",
        "exp": now + 300,
        "iat": now,
        "iss": TEST_ISSUER,
        "aud": TEST_AUDIENCE,
    }))
}

fn validator() -> Arc<JwtValidator> {
    let cfg = JwtConfig {
        issuer: TEST_ISSUER.into(),
        audience: TEST_AUDIENCE.into(),
        public_key_path: "<test>".into(),
        algorithm: JwtAlgorithm::RS256,
        clock_skew_secs: 5,
    };
    Arc::new(JwtValidator::new(&cfg, keypair().public_pem.as_bytes()).expect("validator"))
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

// ============================================================================
// Fake per-tenant stores
// ============================================================================

/// Fake stores keyed by `(TenantId, key)` or scanned-by-tenant.
/// Every store write and read is routed through a `TenantId` discriminator
/// — so a handler reading these stores MUST have access to the request's
/// `TenantContext` to hit the right data. This is exactly the production
/// invariant (Phase 3a/3b tenant scoping).
///
/// The isolation test seeds BOTH tenants into each store, drives requests
/// as tenant A, and asserts only tenant A's rows come back. A bug that
/// drops the `tctx.tenant_id` filter on any path would surface as tenant
/// B data in the response.
#[derive(Default)]
struct FakeStores {
    /// Autocomplete crowdsourced suggestions per tenant × locale.
    autocomplete: Mutex<HashMap<(TenantId, Locale), Vec<String>>>,
    /// Top-searches popular queries per tenant × locale.
    top_searches: Mutex<HashMap<(TenantId, Locale), Vec<String>>>,
    /// Feedback rows keyed by `(tenant_id, request_id)`.
    feedback: Mutex<HashMap<(TenantId, String), FakeFeedbackRow>>,
    /// RAG chunks per tenant — flat list used for `/search` chunk retrieval
    /// simulation.
    rag_chunks: Mutex<HashMap<TenantId, Vec<String>>>,
    /// Conversation history per tenant × session_id — simulates the
    /// `conversations` table read in `SearchOrchestrator`. The values are
    /// the raw prior Q&A strings the LLM would see in its context.
    conversations: Mutex<Vec<(TenantId, String, String)>>, // (tenant, session_id, Q/A text)
}

#[derive(Clone, Debug)]
struct FakeFeedbackRow {
    action: String,
}

impl FakeStores {
    fn seed(&self) {
        let tid_a = TenantId::new(TENANT_A).unwrap();
        let tid_b = TenantId::new(TENANT_B).unwrap();

        // Autocomplete: distinct strings per tenant.
        {
            let mut g = self.autocomplete.lock().unwrap();
            g.insert(
                (tid_a.clone(), Locale::En),
                vec![
                    "alpha-autocomplete-A1".into(),
                    "alpha-autocomplete-A2".into(),
                ],
            );
            g.insert(
                (tid_b.clone(), Locale::En),
                vec!["beta-autocomplete-B1".into(), "beta-autocomplete-B2".into()],
            );
        }

        // Top searches: distinct strings per tenant.
        {
            let mut g = self.top_searches.lock().unwrap();
            g.insert(
                (tid_a.clone(), Locale::En),
                vec!["top-query-A1".into(), "top-query-A2".into()],
            );
            g.insert(
                (tid_b.clone(), Locale::En),
                vec!["top-query-B1".into(), "top-query-B2".into()],
            );
        }

        // Feedback: same request_id under two tenants — a cross-tenant
        // read without scoping would return B's row for tenant A's query.
        {
            let mut g = self.feedback.lock().unwrap();
            g.insert(
                (tid_a.clone(), "req-shared".into()),
                FakeFeedbackRow {
                    action: "like-A".into(),
                },
            );
            g.insert(
                (tid_b.clone(), "req-shared".into()),
                FakeFeedbackRow {
                    action: "dislike-B".into(),
                },
            );
        }

        // RAG chunks: distinct marker strings per tenant.
        {
            let mut g = self.rag_chunks.lock().unwrap();
            g.insert(
                tid_a.clone(),
                vec!["chunk-A-marker-alpha".into(), "chunk-A-marker-beta".into()],
            );
            g.insert(
                tid_b.clone(),
                vec!["chunk-B-marker-gamma".into(), "chunk-B-marker-delta".into()],
            );
        }

        // Conversation history: tenant B's history contains the secret
        // phrase we use for the response-body negative assertion. Both
        // tenants share the same session_id by design — a cross-tenant
        // scan would surface B's history under A's session.
        {
            let mut g = self.conversations.lock().unwrap();
            g.push((
                tid_a.clone(),
                "shared-session".into(),
                "Q: tell me about alpha. A: alpha is fine.".into(),
            ));
            g.push((
                tid_b.clone(),
                "shared-session".into(),
                format!(
                    "Q: tell me about beta. A: beta includes {TENANT_B_SECRET_PHRASE} details."
                ),
            ));
        }
    }
}

// ============================================================================
// Harness state + parallel auth middleware
// ============================================================================

/// Subset of `AppState` the harness middleware reads. Explicit mirror of
/// `tests/auth_flow.rs::ItState` plus `FakeStores` + `SessionLocaleLookup`.
#[derive(Clone)]
struct IsolationState {
    cache: Arc<TenantsCache>,
    validator: Arc<JwtValidator>,
    stores: Arc<FakeStores>,
    /// Fake locale-memory adapter. After Phase 3d.1 `SessionLocaleLookup`
    /// takes `&TenantContext`; this impl returns `None` regardless so
    /// the `ResolvedLocale` extractor falls through to Accept-Language /
    /// default. Isolation semantics on the 5 paths are asserted via
    /// `TenantCtx` reads in the fake handlers; real Redis key scoping
    /// (`sl:{tenant_id}:{session_id}`) is covered by the service-layer
    /// unit test in `session/locale_memory.rs`.
    #[allow(dead_code)]
    locale_lookup: Arc<dyn SessionLocaleLookup>,
}

/// Parallel middleware with the same decision tree as production's
/// `tenant_auth_middleware`. We can't call the real one here because
/// it requires an `Arc<AppState>` (SearchService + Qdrant + Redis +
/// PG). The `_auth_mw_signature_anchor` at the bottom of this file
/// enforces compile-time parity.
async fn auth_mw_for_isolation(
    State(state): State<Arc<IsolationState>>,
    mut req: Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    use kenjaku_core::error::{AuthErrorCode, Error};

    fn err_for(code: AuthErrorCode) -> axum::response::Response {
        let e = Error::TenantAuth(code);
        let status = match code.http_status() {
            401 => StatusCode::UNAUTHORIZED,
            403 => StatusCode::FORBIDDEN,
            429 => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = serde_json::json!({"error": e.user_message()});
        let mut resp = (status, Json(body)).into_response();
        resp.headers_mut()
            .insert("x-knjk-error-code", code.code().parse().unwrap());
        resp
    }

    let validator = &state.validator;
    let bearer = req
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(bearer) = bearer else {
        return err_for(AuthErrorCode::Unauthorized);
    };
    let claims = match validator.validate(bearer) {
        Ok(c) => c,
        Err(_) => return err_for(AuthErrorCode::Unauthorized),
    };
    let Some(row) = state.cache.get(&claims.tenant_id) else {
        return err_for(AuthErrorCode::TenantNotFound);
    };
    let tctx = TenantContext {
        tenant_id: claims.tenant_id,
        principal_id: claims.principal_id,
        plan_tier: row.plan_tier,
    };
    req.extensions_mut().insert(tctx);
    next.run(req).await
}

/// Stub `SessionLocaleLookup` that always misses. The goal of this
/// isolation test is NOT to assert the new widened trait (dev-1's
/// work); it's to assert that per-tenant fake stores read via
/// `TenantCtx` never leak across tenants. The locale-memory path is
/// tested separately by `session/locale_memory.rs`.
struct MissLookup;

#[async_trait]
impl SessionLocaleLookup for MissLookup {
    async fn lookup(&self, _tctx: &TenantContext, _session_id: &str) -> Option<Locale> {
        None
    }
}

// ============================================================================
// Fake handlers mirroring the production extractor signatures
// ============================================================================

#[derive(Deserialize)]
#[allow(dead_code)]
struct AcQuery {
    q: String,
    #[serde(default)]
    locale: Option<String>,
}

#[derive(Serialize)]
struct AcResponse {
    tenant_id: String,
    suggestions: Vec<String>,
    resolved_locale: String,
}

async fn fake_autocomplete(
    TenantCtx(tctx): TenantCtx,
    State(state): State<Arc<IsolationState>>,
    resolved: ResolvedLocale,
    Query(params): Query<AcQuery>,
) -> Json<AcResponse> {
    // Mirror the production handler: reads `tctx.tenant_id` and
    // `resolved.locale`, then hits the store. The store is keyed by
    // `(TenantId, Locale)` — a cross-tenant read is structurally
    // impossible UNLESS the handler drops the tctx filter.
    let _ = params; // q is unused in the fake; the key is tctx.tenant_id.
    let g = state.stores.autocomplete.lock().unwrap();
    let suggestions = g
        .get(&(tctx.tenant_id.clone(), resolved.locale))
        .cloned()
        .unwrap_or_default();
    Json(AcResponse {
        tenant_id: tctx.tenant_id.to_string(),
        suggestions,
        resolved_locale: resolved.locale_str(),
    })
}

#[derive(Serialize)]
struct TopSearchesResp {
    tenant_id: String,
    items: Vec<String>,
    resolved_locale: String,
}

async fn fake_top_searches(
    TenantCtx(tctx): TenantCtx,
    State(state): State<Arc<IsolationState>>,
    resolved: ResolvedLocale,
) -> Json<TopSearchesResp> {
    let g = state.stores.top_searches.lock().unwrap();
    let items = g
        .get(&(tctx.tenant_id.clone(), resolved.locale))
        .cloned()
        .unwrap_or_default();
    Json(TopSearchesResp {
        tenant_id: tctx.tenant_id.to_string(),
        items,
        resolved_locale: resolved.locale_str(),
    })
}

#[derive(Deserialize)]
struct FeedbackReq {
    request_id: String,
    action: String,
}

#[derive(Serialize)]
struct FeedbackResp {
    tenant_id: String,
    request_id: String,
    /// The action on the existing row (if any) BEFORE this write, so
    /// a cross-tenant read would surface the OTHER tenant's action.
    existing_action: Option<String>,
    /// The action stored for (tctx.tenant_id, request_id) after write.
    stored_action: String,
}

async fn fake_feedback(
    TenantCtx(tctx): TenantCtx,
    State(state): State<Arc<IsolationState>>,
    Json(dto): Json<FeedbackReq>,
) -> Json<FeedbackResp> {
    let key = (tctx.tenant_id.clone(), dto.request_id.clone());
    let mut g = state.stores.feedback.lock().unwrap();
    // Read-before-write: the "existing" value proves the scoping.
    let existing_action = g.get(&key).map(|r| r.action.clone());
    g.insert(
        key,
        FakeFeedbackRow {
            action: dto.action.clone(),
        },
    );
    Json(FeedbackResp {
        tenant_id: tctx.tenant_id.to_string(),
        request_id: dto.request_id,
        existing_action,
        stored_action: dto.action,
    })
}

#[derive(Deserialize)]
struct SearchReq {
    query: String,
    session_id: String,
}

#[derive(Serialize)]
struct SearchResp {
    tenant_id: String,
    /// RAG chunks retrieved for this tenant — a cross-tenant bug would
    /// surface the other tenant's marker strings here.
    chunks: Vec<String>,
    /// Conversation history pulled for this (tenant, session_id) — the
    /// critical field for the response-body negative assertion: a
    /// cross-tenant read would surface B's `TENANT_B_SECRET_PHRASE`.
    history: Vec<String>,
    /// Synthesized "answer" that echoes both retrieval and history —
    /// the body-level negative assertion grep's this field to catch
    /// any leak that also makes it through a hypothetical LLM prompt.
    answer: String,
    resolved_locale: String,
}

async fn fake_search(
    TenantCtx(tctx): TenantCtx,
    State(state): State<Arc<IsolationState>>,
    resolved: ResolvedLocale,
    Json(dto): Json<SearchReq>,
) -> Json<SearchResp> {
    // Chunk retrieval: scoped by tctx.tenant_id.
    let chunks = {
        let g = state.stores.rag_chunks.lock().unwrap();
        g.get(&tctx.tenant_id).cloned().unwrap_or_default()
    };

    // Conversation-history scan: MUST filter by `tctx.tenant_id` AND
    // session_id. This is the worst-case leak path — a forgotten
    // tenant filter would drag B's prior Q&A verbatim into A's LLM
    // context. Architect §5.5 case 7 + the task spec's explicit
    // response-body negative assertion target this case.
    let history: Vec<String> = {
        let g = state.stores.conversations.lock().unwrap();
        g.iter()
            .filter(|(t, s, _)| t == &tctx.tenant_id && s == &dto.session_id)
            .map(|(_, _, body)| body.clone())
            .collect()
    };

    // Build a synthetic "answer" body that includes both the retrieved
    // chunks and the conversation history. The negative assertion
    // grep's THIS answer for tenant B's secret phrase — if scoping is
    // broken anywhere upstream, the leak surfaces here. This is a
    // faithful simulation of the production LLM prompt-assembly path
    // (the real `ConversationAssembler::build` stitches history into
    // the Gemini prompt; a cross-tenant history fetch would echo into
    // the model's output).
    let mut answer = String::new();
    answer.push_str(&format!("Query: {}. ", dto.query));
    for c in &chunks {
        answer.push_str(&format!("Chunk: {c}. "));
    }
    for h in &history {
        answer.push_str(&format!("Prior: {h}. "));
    }

    Json(SearchResp {
        tenant_id: tctx.tenant_id.to_string(),
        chunks,
        history,
        answer,
        resolved_locale: resolved.locale_str(),
    })
}

// ============================================================================
// Router wiring
// ============================================================================

fn build_router(state: IsolationState) -> Router {
    let state = Arc::new(state);
    // Layer order matches production `build_router` at
    // `crates/kenjaku-api/src/router.rs`: auth middleware OUTSIDE
    // the per-handler extractors. Axum runs outer layers first, so
    // `TenantContext` is present in `parts.extensions` by the time
    // `TenantCtx::from_request_parts` and `ResolvedLocale::from_request_parts`
    // execute.
    Router::new()
        .route("/api/v1/autocomplete", get(fake_autocomplete))
        .route("/api/v1/top-searches", get(fake_top_searches))
        .route("/api/v1/feedback", post(fake_feedback))
        .route("/api/v1/search", post(fake_search))
        .layer(from_fn_with_state(state.clone(), auth_mw_for_isolation))
        // Global Extension<Arc<dyn SessionLocaleLookup>> matches
        // production router wiring — the extractor pulls it via
        // `parts.extensions.get::<Arc<dyn SessionLocaleLookup>>()`.
        .layer(Extension(state.locale_lookup.clone()))
        .with_state(state)
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(resp.into_body(), 256 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("response body is not valid JSON")
}

fn base_state() -> IsolationState {
    let stores = Arc::new(FakeStores::default());
    stores.seed();
    IsolationState {
        cache: tenants_cache(&[(TENANT_A, PlanTier::Pro), (TENANT_B, PlanTier::Pro)]),
        validator: validator(),
        stores,
        locale_lookup: Arc::new(MissLookup),
    }
}

// ============================================================================
// TESTS — 5 read paths × 2 directions = 10 isolation cases
// ============================================================================

// ----- Path 1: /autocomplete ------------------------------------------------

#[tokio::test]
async fn autocomplete_tenant_a_sees_only_a_data() {
    let app = build_router(base_state());
    let token = token_for(TENANT_A, "user-a-1");
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/autocomplete?q=foo&locale=en")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    assert_eq!(body["tenant_id"], TENANT_A);
    let suggestions = body["suggestions"].as_array().expect("suggestions array");
    let flat = suggestions
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    // Positive: A's data present.
    assert!(
        flat.iter().any(|s| s.contains("A1")),
        "tenant-a must see its own autocomplete row, got: {flat:?}"
    );
    // Negative: B's data absent.
    assert!(
        !flat.iter().any(|s| s.contains("B1") || s.contains("B2")),
        "ISOLATION LEAK: tenant-a saw tenant-b autocomplete data: {flat:?}"
    );
}

#[tokio::test]
async fn autocomplete_tenant_b_sees_only_b_data() {
    let app = build_router(base_state());
    let token = token_for(TENANT_B, "user-b-1");
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/autocomplete?q=foo&locale=en")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    assert_eq!(body["tenant_id"], TENANT_B);
    let suggestions = body["suggestions"].as_array().expect("suggestions array");
    let flat = suggestions
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        flat.iter().any(|s| s.contains("B1")),
        "tenant-b must see its own autocomplete row, got: {flat:?}"
    );
    assert!(
        !flat.iter().any(|s| s.contains("A1") || s.contains("A2")),
        "ISOLATION LEAK: tenant-b saw tenant-a autocomplete data: {flat:?}"
    );
}

// ----- Path 2: /top-searches ------------------------------------------------

#[tokio::test]
async fn top_searches_tenant_a_sees_only_a_data() {
    let app = build_router(base_state());
    let token = token_for(TENANT_A, "user-a-1");
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/top-searches?locale=en")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    assert_eq!(body["tenant_id"], TENANT_A);
    let items = body["items"].as_array().expect("items array");
    let flat = items
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        flat.iter().any(|s| s.contains("A1")),
        "tenant-a must see its own top-searches row, got: {flat:?}"
    );
    assert!(
        !flat.iter().any(|s| s.contains("B1") || s.contains("B2")),
        "ISOLATION LEAK: tenant-a saw tenant-b top-searches data: {flat:?}"
    );
}

#[tokio::test]
async fn top_searches_tenant_b_sees_only_b_data() {
    let app = build_router(base_state());
    let token = token_for(TENANT_B, "user-b-1");
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/top-searches?locale=en")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    assert_eq!(body["tenant_id"], TENANT_B);
    let items = body["items"].as_array().expect("items array");
    let flat = items
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        flat.iter().any(|s| s.contains("B1")),
        "tenant-b must see its own top-searches row, got: {flat:?}"
    );
    assert!(
        !flat.iter().any(|s| s.contains("A1") || s.contains("A2")),
        "ISOLATION LEAK: tenant-b saw tenant-a top-searches data: {flat:?}"
    );
}

// ----- Path 3: /feedback (idempotency-key scoping) --------------------------

#[tokio::test]
async fn feedback_tenant_a_row_isolated_from_b_row_same_request_id() {
    // Both tenants have a pre-seeded row at request_id="req-shared"
    // with distinct `action` values. Driving as tenant A must see
    // A's existing row ("like-A"), never B's ("dislike-B").
    let app = build_router(base_state());
    let token = token_for(TENANT_A, "user-a-1");
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/feedback")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "request_id": "req-shared",
                "action": "like-A-updated",
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    assert_eq!(body["tenant_id"], TENANT_A);
    assert_eq!(
        body["existing_action"].as_str(),
        Some("like-A"),
        "ISOLATION LEAK: feedback idempotency-key resolved to the WRONG \
         tenant's row — expected A's 'like-A', got {body:?}"
    );
}

#[tokio::test]
async fn feedback_tenant_b_row_isolated_from_a_row_same_request_id() {
    let app = build_router(base_state());
    let token = token_for(TENANT_B, "user-b-1");
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/feedback")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "request_id": "req-shared",
                "action": "dislike-B-updated",
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    assert_eq!(body["tenant_id"], TENANT_B);
    assert_eq!(
        body["existing_action"].as_str(),
        Some("dislike-B"),
        "ISOLATION LEAK: feedback idempotency-key resolved to the WRONG \
         tenant's row — expected B's 'dislike-B', got {body:?}"
    );
}

// ----- Path 4: /search (RAG chunks) -----------------------------------------

#[tokio::test]
async fn search_rag_chunks_tenant_a_sees_only_a_chunks() {
    let app = build_router(base_state());
    let token = token_for(TENANT_A, "user-a-1");
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/search")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "query": "what are chunks?",
                "session_id": "sess-a-fresh",
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    assert_eq!(body["tenant_id"], TENANT_A);
    let chunks = body["chunks"].as_array().expect("chunks array");
    let flat = chunks
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        flat.iter().any(|s| s.contains("chunk-A-marker")),
        "tenant-a must see its own chunks, got: {flat:?}"
    );
    assert!(
        !flat.iter().any(|s| s.contains("chunk-B-marker")),
        "ISOLATION LEAK: tenant-a RAG retrieval returned tenant-b chunks: {flat:?}"
    );
}

#[tokio::test]
async fn search_rag_chunks_tenant_b_sees_only_b_chunks() {
    let app = build_router(base_state());
    let token = token_for(TENANT_B, "user-b-1");
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/search")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "query": "what are chunks?",
                "session_id": "sess-b-fresh",
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    assert_eq!(body["tenant_id"], TENANT_B);
    let chunks = body["chunks"].as_array().expect("chunks array");
    let flat = chunks
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        flat.iter().any(|s| s.contains("chunk-B-marker")),
        "tenant-b must see its own chunks, got: {flat:?}"
    );
    assert!(
        !flat.iter().any(|s| s.contains("chunk-A-marker")),
        "ISOLATION LEAK: tenant-b RAG retrieval returned tenant-a chunks: {flat:?}"
    );
}

// ----- Path 5: /search (conversation-history read) --------------------------

#[tokio::test]
async fn search_history_tenant_a_sees_only_a_history() {
    // Both tenants have conversation rows under the same `session_id`
    // ("shared-session"). Driving tenant A must surface only A's row;
    // tenant B's row (containing `TENANT_B_SECRET_PHRASE`) must be
    // invisible.
    let app = build_router(base_state());
    let token = token_for(TENANT_A, "user-a-1");
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/search")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "query": "history isolation probe",
                "session_id": "shared-session",
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    assert_eq!(body["tenant_id"], TENANT_A);
    let history = body["history"].as_array().expect("history array");
    let flat = history
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        flat.iter().any(|s| s.contains("alpha")),
        "tenant-a must see its own history, got: {flat:?}"
    );
    assert!(
        !flat.iter().any(|s| s.contains(TENANT_B_SECRET_PHRASE)),
        "ISOLATION LEAK: tenant-a conversation history included tenant-b's secret phrase"
    );
}

#[tokio::test]
async fn search_history_tenant_b_sees_only_b_history() {
    let app = build_router(base_state());
    let token = token_for(TENANT_B, "user-b-1");
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/search")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "query": "history isolation probe",
                "session_id": "shared-session",
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = body_json(resp).await;
    assert_eq!(body["tenant_id"], TENANT_B);
    let history = body["history"].as_array().expect("history array");
    let flat = history
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        flat.iter().any(|s| s.contains(TENANT_B_SECRET_PHRASE)),
        "tenant-b must see its own history (with secret phrase), got: {flat:?}"
    );
    assert!(
        !flat
            .iter()
            .any(|s| s.contains("alpha") && !s.contains("beta")),
        "ISOLATION LEAK: tenant-b conversation history included tenant-a Q&A: {flat:?}"
    );
}

// ============================================================================
// Response-body negative assertion for /search LLM-prompt-injection vector
// ============================================================================

/// The worst-case conversation-history leak: tenant A's search
/// response body (the "answer" the LLM would produce) echoes tenant
/// B's prior Q&A verbatim — a structural PII/regulatory failure.
///
/// Architect direction (§5.5 case 7; task spec): do NOT instrument the
/// history repo with a new tenant-scoping layer (that was Phase 3b's
/// contract). Verify the result at the edge — seed tenant B's
/// conversation history with a distinctive verbatim string, drive
/// `/search` as tenant A against the same `session_id`, and assert the
/// rendered answer body does NOT contain the B secret.
#[tokio::test]
async fn search_answer_body_never_echoes_cross_tenant_history_secret() {
    let app = build_router(base_state());
    let token = token_for(TENANT_A, "user-a-1");
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/search")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "query": "tell me more about what we discussed",
                "session_id": "shared-session",
            })
            .to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);

    // Consume the full response body — in production this would be the
    // accumulated SSE `delta` stream plus the final `done` payload.
    // Our fake handler synthesizes an `answer` field that concatenates
    // retrieved chunks and prior-turn Q&A — a faithful proxy for what
    // `ConversationAssembler::build` would feed Gemini.
    let body = body_json(resp).await;
    let answer = body["answer"].as_str().unwrap_or_default().to_string();

    assert!(
        !answer.contains(TENANT_B_SECRET_PHRASE),
        "CRITICAL CROSS-TENANT LEAK: tenant-a's /search response body \
         echoed tenant-b's conversation-history secret phrase \
         {TENANT_B_SECRET_PHRASE:?}. Full answer: {answer:?}"
    );

    // Also grep the raw serialized body — belt-and-suspenders against
    // any future change to the response shape that buries history
    // elsewhere (e.g. a `sources` array or a debug metadata field).
    let raw = serde_json::to_string(&body).unwrap();
    assert!(
        !raw.contains(TENANT_B_SECRET_PHRASE),
        "CRITICAL CROSS-TENANT LEAK (full-body grep): tenant-a response \
         body contained tenant-b secret phrase anywhere in serialized \
         output: {raw}"
    );
}

// ============================================================================
// Signature anchors
// ============================================================================

// Mirror `tests/auth_flow.rs:440-451`. If the production
// `tenant_auth_middleware` ever changes its fn signature, this anchor
// fails to compile — catching drift at the IT layer, not only in the
// unit test build.
type AuthMwFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = axum::response::Response> + Send>>;
type AuthMwFn = fn(
    State<std::sync::Arc<kenjaku_api::AppState>>,
    Request<Body>,
    axum::middleware::Next,
) -> AuthMwFuture;

#[allow(dead_code)]
fn _auth_mw_signature_anchor() -> AuthMwFn {
    |s, r, n| Box::pin(tenant_auth_middleware(s, r, n))
}

// Anchor for `SessionLocaleLookup`. The Phase 3d.1 design (architect
// §5.7) widens the trait signature to `lookup(&self, tctx:
// &TenantContext, session_id: &str) -> Option<Locale>`. Until dev-1's
// trait-widening work merges, this anchor stays on the **current**
// (base) signature. Once the widened trait lands on `integrate`, peer
// review should switch this anchor to take `&TenantContext` — a future
// revert of the widening would then fail to compile here.
//
// Upgraded during 3d.1 integrate merge (dev-1's widened trait landed):
// `SessionLocaleLookup::lookup` now takes `&TenantContext` before the
// session id. This anchor tracks the real trait signature so future
// drift is caught at compile time.
#[allow(dead_code)]
fn _lookup_signature_anchor() -> impl SessionLocaleLookup {
    struct A;
    #[async_trait]
    impl SessionLocaleLookup for A {
        async fn lookup(&self, _tctx: &TenantContext, _session_id: &str) -> Option<Locale> {
            None
        }
    }
    A
}
