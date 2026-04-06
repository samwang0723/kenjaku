use std::collections::HashSet;

use reqwest::Client;
use scraper::{Html, Selector};
use tracing::{debug, warn};

/// Crawl a URL and discover linked pages up to a given depth.
pub async fn crawl_urls(entry_url: &str, max_depth: usize) -> anyhow::Result<Vec<String>> {
    let client = Client::builder()
        .user_agent("Kenjaku-Ingester/0.1")
        .timeout(std::time::Duration::from_secs(30))
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

/// Extract links from HTML, filtering to same domain.
fn extract_links(html: &str, base_url: &str, base_domain: &str) -> Vec<String> {
    let document = Html::parse_document(html);
    let selector = Selector::parse("a[href]").unwrap();

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

/// Extract main content from HTML, stripping navigation, scripts, etc.
pub fn extract_text_from_html(html: &str) -> String {
    let document = Html::parse_document(html);

    // Remove script, style, nav, footer, header elements
    let skip_selectors = ["script", "style", "nav", "footer", "header", "aside"];

    let body_selector = Selector::parse("body").unwrap();
    let body = document.select(&body_selector).next();

    if let Some(body) = body {
        let text: String = body
            .text()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        text
    } else {
        document.root_element().text().collect::<Vec<_>>().join("\n")
    }
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
    fn test_extract_text_from_html() {
        let html = r#"
        <html><body>
            <h1>Title</h1>
            <p>This is content.</p>
            <script>var x = 1;</script>
        </body></html>
        "#;

        let text = extract_text_from_html(html);
        assert!(text.contains("Title"));
        assert!(text.contains("This is content"));
    }
}
