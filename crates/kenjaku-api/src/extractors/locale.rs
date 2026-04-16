//! `ResolvedLocale` — Axum extractor implementing the spec §4.7 resolution chain:
//!
//! 1. `?locale=` query param (highest priority, explicit user override)
//! 2. Session memory — looked up via `SessionLocaleLookup` extension when the
//!    request carries an `X-Session-Id` header (or `?session_id=` query param).
//!    The lookup is **tenant-scoped**: the extractor reads
//!    `Extension<TenantContext>` (populated by the auth middleware) and passes
//!    it through to `SessionLocaleLookup::lookup`, so a session id collision
//!    across tenants cannot cross-read locale memory.
//! 3. `Accept-Language` header parsed via the `accept-language` crate, mapped
//!    to a supported [`Locale`] via [`DetectedLocale::from_bcp47`].
//! 4. [`Locale::En`] fallback.
//!
//! # Fail-closed on missing `TenantContext`
//!
//! Unlike steps 1–4 which never fail, the extractor **fails closed with 500**
//! (`LocaleRejection::MissingTenantContext`) if `Extension<TenantContext>` is
//! absent from request extensions. That means the auth middleware didn't run
//! — a programmer error (handler slipped outside the global auth layer), and
//! silently falling back to a hard-coded `public` tenant would resurrect the
//! exact cross-tenant-locale-leak bug 3d.1 was landed to eliminate.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use kenjaku_core::types::locale::{DetectedLocale, Locale};
use kenjaku_core::types::tenant::TenantContext;

/// Provenance of the resolved locale, serialized as snake_case strings matching
/// the DTO contract locked in `architect.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedLocaleSource {
    QueryParam,
    SessionMemory,
    AcceptLanguage,
    Default,
}

impl ResolvedLocaleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QueryParam => "query_param",
            Self::SessionMemory => "session_memory",
            Self::AcceptLanguage => "accept_language",
            Self::Default => "default",
        }
    }
}

/// Trait implemented by the service layer's `LocaleMemory`. Kept local to
/// `kenjaku-api` as a narrow contract so the API crate does not take a direct
/// dependency on concrete service types; the server binary inserts an
/// `Extension<Arc<dyn SessionLocaleLookup>>` into each request for the
/// extractor to pick up.
///
/// # Tenant scoping (Phase 3d.1)
///
/// The `tctx` parameter is threaded from the auth-middleware-populated
/// `Extension<TenantContext>`. Implementations MUST key storage by
/// `(tenant_id, session_id)` — a bare `session_id` key is a cross-tenant
/// leak once tenancy is enabled. See
/// `kenjaku_service::session::locale_memory::LocaleMemory` for the canonical
/// implementation.
#[async_trait]
pub trait SessionLocaleLookup: Send + Sync + 'static {
    async fn lookup(&self, tctx: &TenantContext, session_id: &str) -> Option<Locale>;
}

/// Rejection type for the `ResolvedLocale` extractor.
///
/// Fail-closed by design: missing `Extension<TenantContext>` is treated as
/// a programmer error (middleware didn't run) and surfaces as 500. This
/// mirrors `TenantCtx`'s rejection and defends against a future handler or
/// router change that silently routes around the auth layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocaleRejection {
    /// `Extension<TenantContext>` was not present in request extensions.
    /// The auth middleware did not run for this route, or a route was
    /// attached outside the global auth layer — both programmer errors.
    MissingTenantContext,
}

impl IntoResponse for LocaleRejection {
    fn into_response(self) -> Response {
        match self {
            Self::MissingTenantContext => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}

/// Resolved locale handed to API handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedLocale {
    pub locale: Locale,
    pub source: ResolvedLocaleSource,
}

impl ResolvedLocale {
    pub fn new(locale: Locale, source: ResolvedLocaleSource) -> Self {
        Self { locale, source }
    }

    pub fn locale_str(&self) -> String {
        self.locale.to_string()
    }

    pub fn source_str(&self) -> &'static str {
        self.source.as_str()
    }

    /// Pure resolution from the pieces an Axum extractor would collect. Split
    /// out of the `FromRequestParts` impl so the logic is unit-testable without
    /// building an entire `http::Request`.
    ///
    /// Phase 3d.1: `tctx` is required so the session-memory lookup is
    /// tenant-scoped. Callers in tests can pass `&TenantContext::public()`;
    /// in production the value comes from `Extension<TenantContext>` inserted
    /// by the auth middleware.
    pub async fn resolve(
        tctx: &TenantContext,
        query_locale: Option<&str>,
        session_id: Option<&str>,
        lookup: Option<&Arc<dyn SessionLocaleLookup>>,
        accept_language: Option<&str>,
    ) -> Self {
        // 1. Explicit query param override.
        if let Some(q) = query_locale.map(str::trim).filter(|s| !s.is_empty())
            && let DetectedLocale::Supported(l) = DetectedLocale::from_bcp47(q)
        {
            return Self::new(l, ResolvedLocaleSource::QueryParam);
        }

        // 2. Session memory. Cap session id at 128 chars to prevent Redis
        //    key-size amplification / abuse via arbitrarily long headers.
        if let (Some(id), Some(lk)) = (
            session_id
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.len() <= 128),
            lookup,
        ) && let Some(l) = lk.lookup(tctx, id).await
        {
            return Self::new(l, ResolvedLocaleSource::SessionMemory);
        }

        // 3. Accept-Language header.
        if let Some(header) = accept_language.map(str::trim).filter(|s| !s.is_empty()) {
            // `parse()` returns tags ordered by q-value descending.
            for tag in accept_language::parse(header) {
                if let DetectedLocale::Supported(l) = DetectedLocale::from_bcp47(&tag) {
                    return Self::new(l, ResolvedLocaleSource::AcceptLanguage);
                }
            }
        }

        // 4. Default.
        Self::new(Locale::En, ResolvedLocaleSource::Default)
    }
}

fn query_param<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    let q = query?;
    for pair in q.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        if k == key {
            return it.next();
        }
    }
    None
}

impl<S> FromRequestParts<S> for ResolvedLocale
where
    S: Send + Sync,
{
    type Rejection = LocaleRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Fail-closed: the auth middleware populates `TenantContext`. If it's
        // missing the route is mis-wired (no auth layer), and a silent
        // fallback to `public` would reintroduce the 3d.1 cross-tenant leak.
        let tctx = parts
            .extensions
            .get::<TenantContext>()
            .cloned()
            .ok_or(LocaleRejection::MissingTenantContext)?;

        let query = parts.uri.query();
        // Note: decoded form is good enough for bcp47 tags which are ASCII.
        let query_locale = query_param(query, "locale").map(str::to_owned);
        let query_session = query_param(query, "session_id").map(str::to_owned);

        let header_session = parts
            .headers
            .get("x-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let session_id = header_session.or(query_session);

        let accept_language = parts
            .headers
            .get(axum::http::header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        let lookup = parts
            .extensions
            .get::<Arc<dyn SessionLocaleLookup>>()
            .cloned();

        Ok(ResolvedLocale::resolve(
            &tctx,
            query_locale.as_deref(),
            session_id.as_deref(),
            lookup.as_ref(),
            accept_language.as_deref(),
        )
        .await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;

    use axum::http::Request;
    use kenjaku_core::types::tenant::{TenantContext, TenantId};

    /// Stub that records the `(tenant_id, session_id)` it was called with,
    /// so tests can assert the tctx flows through.
    struct StubLookup {
        response: Option<Locale>,
        last_call: Mutex<Option<(String, String)>>,
    }

    #[async_trait]
    impl SessionLocaleLookup for StubLookup {
        async fn lookup(&self, tctx: &TenantContext, session_id: &str) -> Option<Locale> {
            *self.last_call.lock().unwrap() =
                Some((tctx.tenant_id.as_str().to_string(), session_id.to_string()));
            self.response
        }
    }

    fn stub(response: Option<Locale>) -> Arc<StubLookup> {
        Arc::new(StubLookup {
            response,
            last_call: Mutex::new(None),
        })
    }

    fn lookup(l: Option<Locale>) -> Arc<dyn SessionLocaleLookup> {
        stub(l)
    }

    fn public_tctx() -> TenantContext {
        TenantContext::public()
    }

    fn acme_tctx() -> TenantContext {
        TenantContext {
            tenant_id: TenantId::new("acme").unwrap(),
            principal_id: None,
            plan_tier: kenjaku_core::types::tenant::PlanTier::Pro,
        }
    }

    #[tokio::test]
    async fn resolves_query_param_first() {
        let lk = lookup(Some(Locale::Ko));
        let tctx = public_tctx();
        let r =
            ResolvedLocale::resolve(&tctx, Some("ja"), Some("abc"), Some(&lk), Some("de")).await;
        assert_eq!(r.locale, Locale::Ja);
        assert_eq!(r.source, ResolvedLocaleSource::QueryParam);
    }

    #[tokio::test]
    async fn invalid_query_param_falls_through_to_session_memory() {
        let lk = lookup(Some(Locale::Ko));
        let tctx = public_tctx();
        let r =
            ResolvedLocale::resolve(&tctx, Some("pt"), Some("abc"), Some(&lk), Some("de")).await;
        assert_eq!(r.locale, Locale::Ko);
        assert_eq!(r.source, ResolvedLocaleSource::SessionMemory);
    }

    #[tokio::test]
    async fn resolves_accept_language_when_no_session() {
        let tctx = public_tctx();
        let r =
            ResolvedLocale::resolve(&tctx, None, None, None, Some("ja-JP,ja;q=0.9,en;q=0.7")).await;
        assert_eq!(r.locale, Locale::Ja);
        assert_eq!(r.source, ResolvedLocaleSource::AcceptLanguage);
    }

    #[tokio::test]
    async fn accept_language_skips_unsupported_then_picks_supported() {
        // `pt` is unsupported; must fall through to `de`.
        let tctx = public_tctx();
        let r = ResolvedLocale::resolve(&tctx, None, None, None, Some("pt,de;q=0.9")).await;
        assert_eq!(r.locale, Locale::De);
        assert_eq!(r.source, ResolvedLocaleSource::AcceptLanguage);
    }

    #[tokio::test]
    async fn session_id_without_lookup_falls_through() {
        let tctx = public_tctx();
        let r = ResolvedLocale::resolve(&tctx, None, Some("abc"), None, Some("fr")).await;
        assert_eq!(r.locale, Locale::Fr);
        assert_eq!(r.source, ResolvedLocaleSource::AcceptLanguage);
    }

    #[tokio::test]
    async fn defaults_to_english_when_nothing_present() {
        let tctx = public_tctx();
        let r = ResolvedLocale::resolve(&tctx, None, None, None, None).await;
        assert_eq!(r.locale, Locale::En);
        assert_eq!(r.source, ResolvedLocaleSource::Default);
    }

    #[tokio::test]
    async fn overlong_session_id_is_ignored() {
        // 200-char session id should be rejected by the 128-char cap
        // and fall through to Accept-Language instead of hitting lookup.
        let lk = lookup(Some(Locale::Ko));
        let long = "a".repeat(200);
        let tctx = public_tctx();
        let r = ResolvedLocale::resolve(&tctx, None, Some(&long), Some(&lk), Some("fr")).await;
        assert_eq!(r.locale, Locale::Fr);
        assert_eq!(r.source, ResolvedLocaleSource::AcceptLanguage);
    }

    #[tokio::test]
    async fn session_lookup_miss_falls_through_to_accept_language() {
        let lk = lookup(None);
        let tctx = public_tctx();
        let r = ResolvedLocale::resolve(&tctx, None, Some("abc"), Some(&lk), Some("ko")).await;
        assert_eq!(r.locale, Locale::Ko);
        assert_eq!(r.source, ResolvedLocaleSource::AcceptLanguage);
    }

    #[tokio::test]
    async fn source_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&ResolvedLocaleSource::QueryParam).unwrap(),
            "\"query_param\""
        );
        assert_eq!(
            serde_json::to_string(&ResolvedLocaleSource::SessionMemory).unwrap(),
            "\"session_memory\""
        );
        assert_eq!(
            serde_json::to_string(&ResolvedLocaleSource::AcceptLanguage).unwrap(),
            "\"accept_language\""
        );
        assert_eq!(
            serde_json::to_string(&ResolvedLocaleSource::Default).unwrap(),
            "\"default\""
        );
    }

    #[test]
    fn query_param_parses_multiple_pairs() {
        assert_eq!(
            query_param(Some("foo=1&locale=ja&bar=2"), "locale"),
            Some("ja")
        );
        assert_eq!(query_param(Some("locale=ja"), "session_id"), None);
        assert_eq!(query_param(None, "locale"), None);
    }

    // ---- Phase 3d.1 — tenant-scoped plumbing --------------------------------

    /// Asserts the trait signature widens: `lookup` now takes `&TenantContext`
    /// as its first argument, and the tctx is threaded from `resolve` through
    /// the trait call. Any regression where `resolve` drops the tctx on the
    /// floor will flip the captured tenant_id to `"public"` and fail here.
    #[tokio::test]
    async fn resolve_threads_tenant_context_into_lookup() {
        let stub = stub(Some(Locale::Ja));
        let lk: Arc<dyn SessionLocaleLookup> = stub.clone();
        let tctx = acme_tctx();
        let r = ResolvedLocale::resolve(&tctx, None, Some("sess-1"), Some(&lk), Some("en")).await;
        assert_eq!(r.locale, Locale::Ja);
        assert_eq!(r.source, ResolvedLocaleSource::SessionMemory);
        let call = stub.last_call.lock().unwrap().clone();
        assert_eq!(
            call,
            Some(("acme".to_string(), "sess-1".to_string())),
            "tctx must flow through `resolve` into the lookup impl"
        );
    }

    /// Fail-closed regression: if `Extension<TenantContext>` is missing from
    /// request extensions, the extractor returns a rejection (500) rather
    /// than silently falling back to `public`. Defense-in-depth against a
    /// future route slipping outside the auth layer.
    #[tokio::test]
    async fn from_request_parts_fails_closed_when_tctx_missing() {
        let req = Request::builder()
            .uri("/api/v1/top-searches")
            .body(())
            .unwrap();
        let (mut parts, _) = req.into_parts();
        // Intentionally DO NOT insert TenantContext into extensions.
        let err = <ResolvedLocale as FromRequestParts<()>>::from_request_parts(&mut parts, &())
            .await
            .expect_err("missing TenantContext must fail closed");
        assert_eq!(err, LocaleRejection::MissingTenantContext);
    }

    /// Happy-path: when middleware has populated `TenantContext`, the
    /// extractor threads it through the lookup call, and the resolver
    /// returns the tenant-scoped locale.
    #[tokio::test]
    async fn from_request_parts_uses_extension_tctx() {
        let stub = stub(Some(Locale::Ko));
        let lk: Arc<dyn SessionLocaleLookup> = stub.clone();

        let req = Request::builder()
            .uri("/api/v1/top-searches")
            .header("x-session-id", "sess-abc")
            .body(())
            .unwrap();
        let (mut parts, _) = req.into_parts();
        parts.extensions.insert(acme_tctx());
        parts.extensions.insert(lk);

        let resolved =
            <ResolvedLocale as FromRequestParts<()>>::from_request_parts(&mut parts, &())
                .await
                .unwrap();
        assert_eq!(resolved.locale, Locale::Ko);
        assert_eq!(resolved.source, ResolvedLocaleSource::SessionMemory);

        let call = stub.last_call.lock().unwrap().clone();
        assert_eq!(call, Some(("acme".to_string(), "sess-abc".to_string())));
    }

    #[test]
    fn locale_rejection_missing_tctx_is_500() {
        let resp = LocaleRejection::MissingTenantContext.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
