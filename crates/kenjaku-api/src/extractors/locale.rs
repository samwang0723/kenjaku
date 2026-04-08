//! `ResolvedLocale` — Axum extractor implementing the spec §4.7 resolution chain:
//!
//! 1. `?locale=` query param (highest priority, explicit user override)
//! 2. Session memory — looked up via `SessionLocaleLookup` extension when the
//!    request carries an `X-Session-Id` header (or `?session_id=` query param).
//! 3. `Accept-Language` header parsed via the `accept-language` crate, mapped
//!    to a supported [`Locale`] via [`DetectedLocale::from_bcp47`].
//! 4. [`Locale::En`] fallback.
//!
//! The extractor never fails. Any parse / lookup failure at a step simply
//! falls through to the next one.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde::{Deserialize, Serialize};

use kenjaku_core::types::locale::{DetectedLocale, Locale};

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
#[async_trait]
pub trait SessionLocaleLookup: Send + Sync + 'static {
    async fn lookup(&self, session_id: &str) -> Option<Locale>;
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
    pub async fn resolve(
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

        // 2. Session memory.
        if let (Some(id), Some(lk)) = (session_id.map(str::trim).filter(|s| !s.is_empty()), lookup)
            && let Some(l) = lk.lookup(id).await
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
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
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

    struct StubLookup(Option<Locale>);

    #[async_trait]
    impl SessionLocaleLookup for StubLookup {
        async fn lookup(&self, _session_id: &str) -> Option<Locale> {
            self.0
        }
    }

    fn lookup(l: Option<Locale>) -> Arc<dyn SessionLocaleLookup> {
        Arc::new(StubLookup(l))
    }

    #[tokio::test]
    async fn resolves_query_param_first() {
        let lk = lookup(Some(Locale::Ko));
        let r = ResolvedLocale::resolve(Some("ja"), Some("abc"), Some(&lk), Some("de")).await;
        assert_eq!(r.locale, Locale::Ja);
        assert_eq!(r.source, ResolvedLocaleSource::QueryParam);
    }

    #[tokio::test]
    async fn invalid_query_param_falls_through_to_session_memory() {
        let lk = lookup(Some(Locale::Ko));
        let r = ResolvedLocale::resolve(Some("pt"), Some("abc"), Some(&lk), Some("de")).await;
        assert_eq!(r.locale, Locale::Ko);
        assert_eq!(r.source, ResolvedLocaleSource::SessionMemory);
    }

    #[tokio::test]
    async fn resolves_accept_language_when_no_session() {
        let r = ResolvedLocale::resolve(None, None, None, Some("ja-JP,ja;q=0.9,en;q=0.7")).await;
        assert_eq!(r.locale, Locale::Ja);
        assert_eq!(r.source, ResolvedLocaleSource::AcceptLanguage);
    }

    #[tokio::test]
    async fn accept_language_skips_unsupported_then_picks_supported() {
        // `pt` is unsupported; must fall through to `de`.
        let r = ResolvedLocale::resolve(None, None, None, Some("pt,de;q=0.9")).await;
        assert_eq!(r.locale, Locale::De);
        assert_eq!(r.source, ResolvedLocaleSource::AcceptLanguage);
    }

    #[tokio::test]
    async fn session_id_without_lookup_falls_through() {
        let r = ResolvedLocale::resolve(None, Some("abc"), None, Some("fr")).await;
        assert_eq!(r.locale, Locale::Fr);
        assert_eq!(r.source, ResolvedLocaleSource::AcceptLanguage);
    }

    #[tokio::test]
    async fn defaults_to_english_when_nothing_present() {
        let r = ResolvedLocale::resolve(None, None, None, None).await;
        assert_eq!(r.locale, Locale::En);
        assert_eq!(r.source, ResolvedLocaleSource::Default);
    }

    #[tokio::test]
    async fn session_lookup_miss_falls_through_to_accept_language() {
        let lk = lookup(None);
        let r = ResolvedLocale::resolve(None, Some("abc"), Some(&lk), Some("ko")).await;
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
}
