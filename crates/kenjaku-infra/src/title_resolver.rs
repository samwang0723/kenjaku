//! Resolve real page titles from Gemini grounding redirect URLs.
//!
//! Gemini's google_search grounding returns URIs that point to a
//! `vertexaisearch.cloud.google.com/grounding-api-redirect/...` shim instead
//! of the actual destination. To show users a useful "Sources" panel we need
//! to follow the redirect, fetch the first chunk of HTML, and extract a
//! human title from `<head>`. Resolutions are cached in Redis.
//!
//! This is a Rust port of the Go reference at
//! `~/Downloads/Untitled` (cdc-internal/search-service title_resolver.go).

use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};

use kenjaku_core::types::search::LlmSource;

/// Hard timeout for a single title fetch. Title resolution must never
/// dominate response latency, so failures fall back fast.
const FETCH_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum bytes read from each fetched page. `<title>`, `og:title`,
/// `twitter:title`, and JSON-LD `headline` all live in `<head>`, so 16 KB
/// is plenty.
const MAX_BODY_BYTES: usize = 16 * 1024;

/// TTL for successful resolutions. Crypto domains repeat heavily across
/// queries, so a long cache pays off.
const TITLE_TTL_OK_SECS: u64 = 24 * 60 * 60;
/// Short TTL for failed resolutions — let persistently-broken URLs cool
/// down without hammering them, but allow recovery on a fresh ingest.
const TITLE_TTL_FAIL_SECS: u64 = 10 * 60;
/// Redis key prefix.
const CACHE_PREFIX: &str = "title:";

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// Resolves redirect URLs to (final URL, page title) and caches results in Redis.
#[derive(Clone)]
pub struct TitleResolver {
    http: Client,
    redis: Option<ConnectionManager>,
}

impl TitleResolver {
    /// Create a resolver. Pass `Some(redis_conn)` to enable caching, or `None`
    /// to disable caching (the resolver still works, just slower).
    pub fn new(redis: Option<ConnectionManager>) -> Self {
        let http = Client::builder()
            .timeout(FETCH_TIMEOUT)
            .user_agent(USER_AGENT)
            // Allow up to 5 redirects so vertexaisearch shim → actual page works.
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { http, redis }
    }

    /// Resolve a batch of grounding sources in parallel. The returned vector
    /// preserves input order. Each entry has its title and URL replaced with
    /// the resolved values when resolution succeeds; on failure the original
    /// URL is kept and a slug-derived or host-derived title is used.
    #[instrument(skip(self, sources), fields(count = sources.len()))]
    pub async fn resolve_batch(&self, sources: Vec<LlmSource>) -> Vec<LlmSource> {
        if sources.is_empty() {
            return sources;
        }
        let resolver = Arc::new(self.clone());
        let futures = sources.into_iter().map(|src| {
            let resolver = resolver.clone();
            async move { resolver.resolve_one(src).await }
        });
        join_all(futures).await
    }

    /// Resolve a single source: cache lookup → HTTP fetch → title parse →
    /// fallback chain.
    async fn resolve_one(&self, src: LlmSource) -> LlmSource {
        let cache_key = src.url.clone();

        if let Some(cached) = self.cache_get(&cache_key).await {
            debug!(url = %src.url, title = %cached.title, "title cache hit");
            return cached;
        }

        let resolved = match self.fetch_and_extract(&src.url).await {
            Ok((final_url, title)) if !title.is_empty() => LlmSource {
                title,
                url: final_url,
                snippet: src.snippet.clone(),
            },
            Ok((final_url, _)) => {
                // Title parse failed: fall back to slug, then host.
                let title = title_from_url_slug(&final_url)
                    .or_else(|| host_of(&final_url))
                    .unwrap_or_else(|| src.title.clone());
                LlmSource {
                    title,
                    url: final_url,
                    snippet: src.snippet.clone(),
                }
            }
            Err(e) => {
                debug!(url = %src.url, error = %e, "title fetch failed");
                let title = title_from_url_slug(&src.url)
                    .or_else(|| host_of(&src.url))
                    .unwrap_or_else(|| src.title.clone());
                LlmSource {
                    title,
                    url: src.url.clone(),
                    snippet: src.snippet.clone(),
                }
            }
        };

        let ttl = if resolved.title.is_empty() {
            TITLE_TTL_FAIL_SECS
        } else {
            TITLE_TTL_OK_SECS
        };
        self.cache_set(&cache_key, &resolved, ttl).await;

        resolved
    }

    /// Fetch the URL and read up to `MAX_BODY_BYTES`. Returns the
    /// post-redirect final URL and any extracted title.
    async fn fetch_and_extract(&self, url: &str) -> Result<(String, String), String> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| format!("fetch error: {e}"))?;

        let final_url = resp.url().to_string();
        if !resp.status().is_success() {
            return Ok((final_url, String::new()));
        }

        // Stream up to MAX_BODY_BYTES — we don't need the full document.
        let mut body = Vec::with_capacity(MAX_BODY_BYTES);
        let mut stream = resp.bytes_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("body read error: {e}"))?;
            let remaining = MAX_BODY_BYTES.saturating_sub(body.len());
            if remaining == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            if body.len() >= MAX_BODY_BYTES {
                break;
            }
        }

        let html = String::from_utf8_lossy(&body);
        let title = extract_title(&html);
        Ok((final_url, title))
    }

    async fn cache_get(&self, key: &str) -> Option<LlmSource> {
        let mut conn = self.redis.as_ref()?.clone();
        let raw: redis::RedisResult<String> = conn.get(format!("{CACHE_PREFIX}{key}")).await;
        let raw = raw.ok()?;
        serde_json::from_str::<CachedTitle>(&raw)
            .ok()
            .map(|c| LlmSource {
                title: c.title,
                url: c.url,
                snippet: None,
            })
    }

    async fn cache_set(&self, key: &str, src: &LlmSource, ttl_secs: u64) {
        let Some(mut conn) = self.redis.as_ref().cloned() else {
            return;
        };
        let payload = CachedTitle {
            url: src.url.clone(),
            title: src.title.clone(),
        };
        let Ok(json) = serde_json::to_string(&payload) else {
            return;
        };
        let _: redis::RedisResult<()> = conn
            .set_ex(format!("{CACHE_PREFIX}{key}"), json, ttl_secs)
            .await;
    }
}

#[derive(Serialize, Deserialize)]
struct CachedTitle {
    #[serde(rename = "u")]
    url: String,
    #[serde(rename = "t")]
    title: String,
}

/// Extract a page title from HTML using a priority cascade: og:title →
/// twitter:title → `<title>` → JSON-LD headline → `<meta name="title">`.
fn extract_title(html: &str) -> String {
    if let Some(t) = extract_meta_content(html, "og:title")
        && !t.is_empty()
    {
        return decode_html_entities(t.trim());
    }
    if let Some(t) = extract_meta_content(html, "twitter:title")
        && !t.is_empty()
    {
        return decode_html_entities(t.trim());
    }
    if let Some(t) = extract_title_tag(html)
        && !t.is_empty()
    {
        return decode_html_entities(t.trim());
    }
    if let Some(t) = extract_jsonld_headline(html)
        && !t.is_empty()
    {
        return decode_html_entities(t.trim());
    }
    if let Some(t) = extract_meta_content(html, "title")
        && !t.is_empty()
    {
        return decode_html_entities(t.trim());
    }
    String::new()
}

/// Extract the contents of `<title>...</title>`. Case-insensitive.
fn extract_title_tag(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let start_tag = lower.find("<title")?;
    let close_gt = lower[start_tag..].find('>')? + start_tag + 1;
    let end_tag = lower[close_gt..].find("</title>")? + close_gt;
    Some(html[close_gt..end_tag].trim().to_string())
}

/// Extract `content="..."` from a `<meta>` tag whose `property=` or `name=`
/// attribute matches `meta_name`. Quote-aware, case-insensitive.
fn extract_meta_content(html: &str, meta_name: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let target = meta_name.to_lowercase();
    let mut from = 0usize;
    while let Some(idx) = lower[from..].find("<meta") {
        let tag_start = from + idx;
        let tag_end = lower[tag_start..].find('>')? + tag_start + 1;
        let tag_lower = &lower[tag_start..tag_end];
        let tag_orig = &html[tag_start..tag_end];

        if (attr_matches(tag_lower, "property", &target)
            || attr_matches(tag_lower, "name", &target))
            && let Some(content) = extract_attr_value(tag_orig, "content")
        {
            return Some(content);
        }
        from = tag_end;
    }
    None
}

fn attr_matches(tag_lower: &str, attr: &str, value_lower: &str) -> bool {
    extract_attr_value(tag_lower, attr)
        .map(|v| v.eq_ignore_ascii_case(value_lower))
        .unwrap_or(false)
}

/// Extract an HTML attribute value (quoted with `"` or `'`).
fn extract_attr_value(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_lowercase();
    let target = attr.to_lowercase();
    let bytes = lower.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;

    while i < n {
        let rest = &lower[i..];
        let idx = rest.find(&target)?;
        let pos = i + idx;

        // Boundary check: char before must be whitespace or '<'.
        let before_ok = pos == 0 || matches!(bytes[pos - 1], b' ' | b'\t' | b'\n' | b'\r' | b'<');
        let end_name = pos + target.len();
        let after_ok =
            end_name >= n || matches!(bytes[end_name], b' ' | b'\t' | b'\n' | b'\r' | b'=');
        if !before_ok || !after_ok {
            i = pos + target.len();
            continue;
        }

        // Skip whitespace, expect '='.
        let mut j = end_name;
        while j < n && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
            j += 1;
        }
        if j >= n || bytes[j] != b'=' {
            i = end_name;
            continue;
        }
        j += 1;
        while j < n && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') {
            j += 1;
        }
        if j >= n {
            return None;
        }
        let quote = tag.as_bytes()[j];
        if quote != b'"' && quote != b'\'' {
            i = j + 1;
            continue;
        }
        let start = j + 1;
        let end = tag[start..].find(quote as char)?;
        return Some(tag[start..start + end].to_string());
    }
    None
}

/// Extract a JSON-LD `headline` field from a `<script type="application/ld+json">`
/// block. Common on news/blog sites.
fn extract_jsonld_headline(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let mut from = 0usize;
    while let Some(idx) = lower[from..].find("application/ld+json") {
        let pos = from + idx;
        let gt = lower[pos..].find('>')? + pos + 1;
        let end = lower[gt..].find("</script>")? + gt;
        let block = &html[gt..end];
        if let Some(h) = extract_json_string_field(block, "headline") {
            return Some(h);
        }
        from = end;
    }
    None
}

/// Read a string-valued JSON field by simple scanning. Handles `\"` and `\\`
/// escapes — sufficient for grounding HTML which is non-adversarial.
fn extract_json_string_field(blob: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let pos = blob.find(&needle)?;
    let rest = &blob[pos + needle.len()..];
    let rest = rest.trim_start_matches([' ', '\t', '\n', '\r']);
    let rest = rest.strip_prefix(':')?;
    let rest = rest.trim_start_matches([' ', '\t', '\n', '\r']);
    let rest = rest.strip_prefix('"')?;

    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            }
            continue;
        }
        if c == '"' {
            return Some(out);
        }
        out.push(c);
    }
    None
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&#x27;", "'")
        .replace("&#039;", "'")
        .replace("&ndash;", "–")
        .replace("&mdash;", "—")
        .replace("&#8211;", "–")
        .replace("&#8212;", "—")
}

/// Convert the last meaningful URL path segment into a title-case string,
/// e.g. `https://x/blog/crypto-market-analysis-2026` → `Crypto Market Analysis 2026`.
/// Returns `None` for paths like `/`, pure IDs, or segments shorter than 10 chars.
fn title_from_url_slug(raw_url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(raw_url).ok()?;
    let path = parsed.path().trim_end_matches('/');
    for seg in path.split('/').rev() {
        if seg.is_empty() || looks_like_id(seg) {
            continue;
        }
        let replaced: String = seg
            .chars()
            .map(|c| if c == '-' || c == '_' { ' ' } else { c })
            .collect();
        let stripped = strip_trailing_id(&replaced);
        let trimmed = stripped.trim();
        if trimmed.len() < 10 {
            continue;
        }
        return Some(title_case(trimmed));
    }
    None
}

fn host_of(raw_url: &str) -> Option<String> {
    reqwest::Url::parse(raw_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
}

fn strip_trailing_id(s: &str) -> String {
    let mut words: Vec<&str> = s.split_whitespace().collect();
    while let Some(last) = words.last() {
        let all_digits = !last.is_empty() && last.chars().all(|c| c.is_ascii_digit());
        if all_digits && last.len() > 4 {
            words.pop();
        } else {
            break;
        }
    }
    words.join(" ")
}

fn looks_like_id(seg: &str) -> bool {
    if seg.len() > 20 {
        let digits = seg.chars().filter(|c| c.is_ascii_digit()).count();
        if digits as f64 / seg.len() as f64 > 0.8 {
            return true;
        }
    }
    !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit())
}

fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_title_tag_basic() {
        let html = "<html><head><title>Hello World</title></head></html>";
        assert_eq!(extract_title_tag(html).unwrap(), "Hello World");
    }

    #[test]
    fn extract_title_tag_with_attrs() {
        let html = "<html><head><title lang=\"en\">Hi</title></head></html>";
        assert_eq!(extract_title_tag(html).unwrap(), "Hi");
    }

    #[test]
    fn extract_meta_og_title() {
        let html = r#"<meta property="og:title" content="Bitcoin Surges">"#;
        assert_eq!(
            extract_meta_content(html, "og:title").unwrap(),
            "Bitcoin Surges"
        );
    }

    #[test]
    fn extract_meta_twitter_title() {
        let html = r#"<meta name='twitter:title' content='Crypto News'>"#;
        assert_eq!(
            extract_meta_content(html, "twitter:title").unwrap(),
            "Crypto News"
        );
    }

    #[test]
    fn cascade_prefers_og_title() {
        let html = r#"<html><head>
            <title>Site Name</title>
            <meta property="og:title" content="Real Article Title">
        </head></html>"#;
        assert_eq!(extract_title(html), "Real Article Title");
    }

    #[test]
    fn cascade_falls_back_to_title_tag() {
        let html = "<html><head><title>Plain Title</title></head></html>";
        assert_eq!(extract_title(html), "Plain Title");
    }

    #[test]
    fn jsonld_headline() {
        let html = r#"<script type="application/ld+json">
        {"@type":"NewsArticle","headline":"Markets Today"}
        </script>"#;
        assert_eq!(extract_jsonld_headline(html).unwrap(), "Markets Today");
    }

    #[test]
    fn html_entities_decoded() {
        let html = "<title>AT&amp;T &mdash; News</title>";
        assert_eq!(extract_title(html), "AT&T — News");
    }

    #[test]
    fn slug_from_url() {
        let title = title_from_url_slug("https://x.com/blog/crypto-market-analysis-2026");
        assert_eq!(title.as_deref(), Some("Crypto Market Analysis 2026"));
    }

    #[test]
    fn slug_strips_trailing_id() {
        let title =
            title_from_url_slug("https://x/wheat-closes-with-report-day-gains-1775004276657");
        assert_eq!(title.as_deref(), Some("Wheat Closes With Report Day Gains"));
    }

    #[test]
    fn slug_skips_short_segments() {
        assert_eq!(title_from_url_slug("https://x.com/a/b/c"), None);
    }
}
