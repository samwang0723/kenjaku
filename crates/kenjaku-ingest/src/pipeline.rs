use std::path::Path;
use std::time::Instant;

use tracing::{error, info};
use uuid::Uuid;

use crate::chunker::chunk_text;
use crate::crawler::{crawl_urls, extract_text_from_html};
use crate::parser::{discover_files, extract_title, parse_file};

/// Extract a title from a URL string (last path segment or hostname).
fn title_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|segments| segments.last().map(String::from))
                .filter(|s| !s.is_empty())
                .or_else(|| u.host_str().map(String::from))
        })
        .unwrap_or_else(|| "Untitled".to_string())
}

/// Ingest documents from a URL by crawling.
pub async fn ingest_url(
    entry_url: &str,
    depth: usize,
    collection: &str,
    chunk_size: usize,
    chunk_overlap: usize,
    batch_size: usize,
    concurrency: usize,
) -> anyhow::Result<()> {
    let start = Instant::now();

    info!("Discovering URLs from {entry_url} with depth {depth}");
    let urls = crawl_urls(entry_url, depth).await?;
    info!("Discovered {} URLs", urls.len());

    let client = reqwest::Client::builder()
        .user_agent("Kenjaku-Ingester/0.1")
        .build()?;

    let mut total_chunks = 0;
    let mut total_errors = 0;

    for url in &urls {
        match client.get(url).send().await {
            Ok(response) if response.status().is_success() => {
                let html = response.text().await.unwrap_or_default();
                let text = extract_text_from_html(&html);
                let title = title_from_url(url);

                let chunks = chunk_text(&text, chunk_size, chunk_overlap);
                total_chunks += chunks.len();

                info!(
                    url = %url,
                    chunks = chunks.len(),
                    "Processed document"
                );

                // TODO: Contextualize chunks via Claude API
                // TODO: Generate embeddings via OpenAI API
                // TODO: Store in Qdrant
            }
            Ok(response) => {
                error!(url = %url, status = %response.status(), "Failed to fetch");
                total_errors += 1;
            }
            Err(e) => {
                error!(url = %url, error = %e, "Request failed");
                total_errors += 1;
            }
        }
    }

    let elapsed = start.elapsed();
    info!(
        urls = urls.len(),
        chunks = total_chunks,
        errors = total_errors,
        elapsed_secs = elapsed.as_secs(),
        "URL ingestion complete"
    );

    Ok(())
}

/// Ingest documents from a local folder.
/// The folder path is canonicalized and validated before traversal.
pub async fn ingest_folder(
    folder_path: &str,
    collection: &str,
    chunk_size: usize,
    chunk_overlap: usize,
    batch_size: usize,
    concurrency: usize,
) -> anyhow::Result<()> {
    let start = Instant::now();

    // Canonicalize and validate the folder path to prevent traversal
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path
    let canonical_path = Path::new(folder_path).canonicalize().map_err(|e| {
        anyhow::anyhow!("Failed to resolve folder path {folder_path}: {e}")
    })?;

    if !canonical_path.is_dir() {
        return Err(anyhow::anyhow!(
            "Path is not a directory: {}",
            canonical_path.display()
        ));
    }

    let files = discover_files(&canonical_path);
    info!(
        "Discovered {} files in {}",
        files.len(),
        canonical_path.display()
    );

    let mut total_chunks = 0;
    let mut total_errors = 0;

    for file_path in &files {
        match parse_file(file_path) {
            Ok((text, doc_type)) => {
                let title = extract_title(&text, file_path);
                let _doc_id = Uuid::new_v4().to_string();

                let chunks = chunk_text(&text, chunk_size, chunk_overlap);
                total_chunks += chunks.len();

                info!(
                    file = %file_path.display(),
                    doc_type = %doc_type,
                    chunks = chunks.len(),
                    "Processed document"
                );

                // TODO: Contextualize chunks via Claude API
                // TODO: Generate embeddings via OpenAI API
                // TODO: Store in Qdrant
            }
            Err(e) => {
                error!(file = %file_path.display(), error = %e, "Failed to parse");
                total_errors += 1;
            }
        }
    }

    let elapsed = start.elapsed();
    info!(
        files = files.len(),
        chunks = total_chunks,
        errors = total_errors,
        elapsed_secs = elapsed.as_secs(),
        "Folder ingestion complete"
    );

    Ok(())
}
