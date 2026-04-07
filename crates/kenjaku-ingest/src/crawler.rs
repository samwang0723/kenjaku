use std::collections::HashSet;
use std::net::IpAddr;

use reqwest::Client;
use scraper::{Html, Selector};
use tracing::{debug, warn};

/// Private/reserved IP ranges that must not be crawled (SSRF protection).
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                // 169.254.0.0/16
                || (v4.octets()[0] == 169 && v4.octets()[1] == 254)
                // 100.64.0.0/10 (carrier-grade NAT)
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

/// Validate that a URL does not point to a private/internal IP.
async fn validate_url_not_private(url_str: &str) -> anyhow::Result<()> {
    let parsed = url::Url::parse(url_str)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("URL has no host"))?;

    let addrs = tokio::net::lookup_host(format!(
        "{}:{}",
        host,
        parsed.port_or_known_default().unwrap_or(80)
    ))
    .await?;

    for addr in addrs {
        if is_private_ip(addr.ip()) {
            anyhow::bail!(
                "SSRF blocked: URL '{}' resolves to private IP {}",
                url_str,
                addr.ip()
            );
        }
    }

    Ok(())
}

/// Crawl a URL and discover linked pages up to a given depth.
pub async fn crawl_urls(entry_url: &str, max_depth: usize) -> anyhow::Result<Vec<String>> {
    validate_url_not_private(entry_url).await?;

    let client = Client::builder()
        .user_agent("Kenjaku-Ingester/0.1")
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let mut visited = HashSet::new();
    let mut to_visit = vec![(entry_url.to_string(), 0_usize)];
    let mut discovered = Vec::new();

    let base_url = url::Url::parse(entry_url)?;
    let base_domain = base_url.domain().unwrap_or("").to_string();

    while let Some((url, depth)) = to_visit.pop() {
        if visited.contains(&url) || depth > max_depth {
            continue;
        }
        visited.insert(url.clone());

        if let Err(e) = validate_url_not_private(&url).await {
            warn!(url = %url, error = %e, "Skipping URL (SSRF check)");
            continue;
        }

        debug!(url = %url, depth = depth, "Crawling");

        match client.get(&url).send().await {
            Ok(response) if response.status().is_success() => {
                let body = response.text().await.unwrap_or_default();
                discovered.push(url.clone());

                if depth < max_depth {
                    let links = extract_links(&body, &url, &base_domain);
                    for link in links {
                        if !visited.contains(&link) {
                            to_visit.push((link, depth + 1));
                        }
                    }
                }
            }
            Ok(response) => {
                warn!(url = %url, status = %response.status(), "Non-success status");
            }
            Err(e) => {
                warn!(url = %url, error = %e, "Failed to fetch");
            }
        }
    }

    Ok(discovered)
}

/// Extract same-domain HTTP(S) links from HTML.
fn extract_links(html: &str, base_url: &str, base_domain: &str) -> Vec<String> {
    let document = Html::parse_document(html);
    let Ok(selector) = Selector::parse("a[href]") else {
        return Vec::new();
    };

    let base = url::Url::parse(base_url).ok();

    document
        .select(&selector)
        .filter_map(|element| {
            let href = element.value().attr("href")?;
            let resolved = if let Some(ref base) = base {
                base.join(href).ok()?.to_string()
            } else {
                href.to_string()
            };

            let parsed = url::Url::parse(&resolved).ok()?;
            let domain = parsed.domain()?;

            if domain == base_domain
                && parsed.scheme().starts_with("http")
                && !resolved.contains('#')
            {
                Some(resolved)
            } else {
                None
            }
        })
        .collect()
}

/// Convert HTML to clean Markdown for RAG indexing.
///
/// 1. Strip noise tags (script, style, nav, footer, header, aside, form, iframe)
/// 2. Extract main content (<main> / <article> / <body>)
/// 3. Convert to markdown via html2md (preserves headings, lists, emphasis)
/// 4. Clean up: collapse blank lines, remove empty link/image artifacts,
///    drop lines that are pure URL/whitespace junk
pub fn extract_text_from_html(html: &str) -> String {
    let cleaned = strip_noise_tags(html);
    let main_html = extract_main_content_html(&cleaned).unwrap_or(cleaned);

    let md = html2md::parse_html(&main_html);
    clean_markdown(&md)
}

/// Extract just the inner HTML of the main content area.
/// Falls back to None if no recognizable content tag is found.
fn extract_main_content_html(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    for sel in ["main", "article", "body"] {
        let Ok(selector) = Selector::parse(sel) else {
            continue;
        };
        if let Some(el) = document.select(&selector).next() {
            return Some(el.inner_html());
        }
    }
    None
}

/// Clean up converted markdown:
/// - Collapse multiple blank lines to one
/// - Drop lines that are only link/image punctuation artifacts
/// - Drop lines that are bare URLs
/// - Trim trailing whitespace
fn clean_markdown(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut blank_count = 0;

    for raw in md.lines() {
        let line = raw.trim_end();

        if is_artifact_line(line) {
            continue;
        }

        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count == 1 {
                out.push('\n');
            }
            continue;
        }

        blank_count = 0;
        out.push_str(line);
        out.push('\n');
    }

    out.trim().to_string()
}

/// A line is "artifact" noise if it's:
/// - only brackets/parens/punctuation (empty markdown link/image)
/// - just a bare URL like "https://..."
/// - just an image marker like "![]"
fn is_artifact_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }

    // Bare URL line (e.g., "https://example.com" or "<https://...>")
    let url_candidate = t.trim_start_matches('<').trim_end_matches('>');
    if (url_candidate.starts_with("http://") || url_candidate.starts_with("https://"))
        && !url_candidate.contains(' ')
    {
        return true;
    }

    // Pure punctuation/bracket noise
    t.chars()
        .all(|c| matches!(c, '[' | ']' | '(' | ')' | '!' | '-' | '*' | ' ' | '|' | '_'))
}

/// Strip script/style/nav/footer/header/aside/form/iframe/noscript tags
/// along with their content. This is a simple tag-matching strip, not a full
/// HTML parser — good enough for known noise tags that rarely nest.
fn strip_noise_tags(html: &str) -> String {
    const NOISE_TAGS: &[&str] = &[
        "script", "style", "nav", "footer", "header", "aside", "form", "iframe", "noscript", "svg",
        "template",
    ];

    let mut result = html.to_string();
    for tag in NOISE_TAGS {
        result = strip_tag(&result, tag);
    }
    result
}

/// Remove all occurrences of `<tag ...>...</tag>` (case-insensitive) from `html`.
fn strip_tag(html: &str, tag: &str) -> String {
    let lower = html.to_lowercase();
    let open_pat = format!("<{tag}");
    let close_pat = format!("</{tag}>");
    let close_len = close_pat.len();

    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;

    while cursor < html.len() {
        // Find next opening tag (must be followed by whitespace or '>')
        let rel = match lower[cursor..].find(&open_pat) {
            Some(i) => i,
            None => {
                out.push_str(&html[cursor..]);
                break;
            }
        };
        let open_start = cursor + rel;

        // Validate it's a real tag boundary, not a prefix match like <scripty>
        let after_open = open_start + open_pat.len();
        if after_open < html.len() {
            let next = html.as_bytes()[after_open];
            if next != b' ' && next != b'>' && next != b'\t' && next != b'\n' && next != b'/' {
                out.push_str(&html[cursor..open_start + 1]);
                cursor = open_start + 1;
                continue;
            }
        }

        // Copy everything before the opening tag
        out.push_str(&html[cursor..open_start]);

        // Find the matching closing tag
        match lower[after_open..].find(&close_pat) {
            Some(close_rel) => {
                cursor = after_open + close_rel + close_len;
            }
            None => {
                // Unclosed tag — skip the rest of the document
                break;
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_links() {
        let html = r##"
        <html><body>
            <a href="/page1">Page 1</a>
            <a href="https://example.com/page2">Page 2</a>
            <a href="https://other.com/page3">External</a>
            <a href="#anchor">Anchor</a>
        </body></html>
        "##;

        let links = extract_links(html, "https://example.com/", "example.com");
        assert!(links.contains(&"https://example.com/page1".to_string()));
        assert!(links.contains(&"https://example.com/page2".to_string()));
        assert!(!links.iter().any(|l| l.contains("other.com")));
        assert!(!links.iter().any(|l| l.contains("#")));
    }

    #[test]
    fn test_strip_noise_tags_script() {
        let html = r#"<html><body><h1>Title</h1><script>var x = {"key":"val"};</script><p>Content</p></body></html>"#;
        let stripped = strip_noise_tags(html);
        assert!(!stripped.contains("var x"));
        assert!(!stripped.contains("<script"));
        assert!(stripped.contains("Title"));
        assert!(stripped.contains("Content"));
    }

    #[test]
    fn test_strip_noise_tags_multiple() {
        let html = r#"
        <html><body>
            <nav>Menu</nav>
            <header>Header</header>
            <main><h1>Main</h1><p>Body text</p></main>
            <aside>Sidebar</aside>
            <footer>Footer</footer>
            <style>.x { color: red; }</style>
        </body></html>
        "#;
        let stripped = strip_noise_tags(html);
        assert!(!stripped.contains("Menu"));
        assert!(!stripped.contains("Sidebar"));
        assert!(!stripped.contains("Footer"));
        assert!(!stripped.contains("color: red"));
        assert!(stripped.contains("Main"));
        assert!(stripped.contains("Body text"));
    }

    #[test]
    fn test_extract_html_to_markdown() {
        let html = r#"
        <html><head><title>Test</title></head>
        <body>
            <nav><a href="/">Home</a></nav>
            <script>var stuff = {"a": 1};</script>
            <main>
                <h1>Welcome</h1>
                <p>This is a <strong>test</strong> paragraph.</p>
                <p>Visit <a href="https://example.com">example</a> for more.</p>
                <ul><li>Item one</li><li>Item two</li></ul>
            </main>
            <footer>Copyright 2026</footer>
        </body></html>
        "#;

        let md = extract_text_from_html(html);

        // Main content preserved (with markdown structure)
        assert!(md.contains("Welcome"));
        assert!(md.contains("test"));
        assert!(md.contains("Item one"));
        assert!(md.contains("Item two"));
        assert!(md.contains("example"));

        // Noise removed
        assert!(!md.contains("var stuff"));
        assert!(!md.contains("Copyright 2026"));
        assert!(!md.contains("Home"));
        assert!(!md.contains("<script"));
    }

    #[test]
    fn test_clean_markdown_drops_artifact_lines() {
        let input = "Real content here.\n[]()\n![]\n\n\nMore content.\n   []   \n";
        let cleaned = clean_markdown(input);
        assert!(cleaned.contains("Real content"));
        assert!(cleaned.contains("More content"));
        assert!(!cleaned.contains("[]()"));
        assert!(!cleaned.contains("![]"));
    }

    #[test]
    fn test_clean_markdown_collapses_blank_lines() {
        let input = "line one\n\n\n\nline two";
        let cleaned = clean_markdown(input);
        assert!(!cleaned.contains("\n\n\n"));
    }

    #[test]
    fn test_is_artifact_line_bare_url() {
        assert!(is_artifact_line("https://example.com"));
        assert!(is_artifact_line("<https://example.com>"));
        assert!(!is_artifact_line("Visit https://example.com for info"));
    }
}
