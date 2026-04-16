use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use tracing::{error, info, warn};
use uuid::Uuid;

use kenjaku_core::config::AppConfig;
use kenjaku_core::traits::embedding::EmbeddingProvider;
use kenjaku_core::traits::llm::Contextualizer;
use kenjaku_infra::embedding::create_embedding_provider;
use kenjaku_infra::llm::ClaudeContextualizer;
use kenjaku_infra::qdrant::{PointData, QdrantClient};

use crate::chunker::chunk_text;
use crate::crawler::{crawl_urls, extract_text_from_html};
use crate::parser::{discover_files, extract_title, parse_file};

/// Clients needed to run the ingest pipeline.
pub struct IngestClients {
    pub qdrant: QdrantClient,
    pub embedder: Arc<dyn EmbeddingProvider>,
    pub contextualizer: Arc<dyn Contextualizer>,
}

impl IngestClients {
    /// Build all clients from the application config.
    pub async fn from_config(config: &AppConfig) -> anyhow::Result<Self> {
        let qdrant = QdrantClient::new(config.qdrant.clone()).await?;
        qdrant.ensure_collection().await?;

        let embedder = Arc::from(create_embedding_provider(config.embedding.clone())?);
        let contextualizer: Arc<dyn Contextualizer> =
            Arc::new(ClaudeContextualizer::new(config.contextualizer.clone()));

        Ok(Self {
            qdrant,
            embedder,
            contextualizer,
        })
    }
}

/// Extract a title from a URL string (last path segment or hostname).
fn title_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut segments| segments.next_back().map(String::from))
                .filter(|s| !s.is_empty())
                .or_else(|| u.host_str().map(String::from))
        })
        .unwrap_or_else(|| "Untitled".to_string())
}

/// Process a single document: chunk -> contextualize -> embed -> store.
#[allow(clippy::too_many_arguments)]
async fn process_document(
    clients: &IngestClients,
    doc_id: &str,
    title: &str,
    full_text: &str,
    source_url: Option<&str>,
    doc_type: &str,
    collection: &str,
    chunk_size: usize,
    chunk_overlap: usize,
    batch_size: usize,
) -> anyhow::Result<usize> {
    let chunks = chunk_text(full_text, chunk_size, chunk_overlap);
    if chunks.is_empty() {
        return Ok(0);
    }

    // Step 1: Contextualize each chunk (Claude with prompt caching).
    // The document content is cached, so only chunk prompts change per call.
    let mut contextualized: Vec<(String, String)> = Vec::with_capacity(chunks.len());
    for chunk in &chunks {
        match clients.contextualizer.contextualize(full_text, chunk).await {
            Ok(ctx) => {
                // Contextualized content = context + original chunk
                let combined = format!("{ctx}\n\n{chunk}");
                contextualized.push((chunk.clone(), combined));
            }
            Err(e) => {
                warn!(error = %e, "Contextualization failed, using raw chunk");
                contextualized.push((chunk.clone(), chunk.clone()));
            }
        }
    }

    // Step 2: Embed in batches.
    let now = Utc::now().to_rfc3339();
    let mut inserted = 0;

    for batch in contextualized.chunks(batch_size) {
        let texts: Vec<String> = batch.iter().map(|(_, ctx)| ctx.clone()).collect();
        let embeddings = clients.embedder.embed(&texts).await?;

        if embeddings.len() != batch.len() {
            return Err(anyhow::anyhow!(
                "Embedding count mismatch: {} chunks -> {} embeddings",
                batch.len(),
                embeddings.len()
            ));
        }

        // Step 3: Build Qdrant points.
        let points: Vec<PointData> = batch
            .iter()
            .zip(embeddings)
            .map(|((original, contextualized), embedding)| PointData {
                point_id: Uuid::new_v4().to_string(),
                doc_id: doc_id.to_string(),
                chunk_id: Uuid::new_v4().to_string(),
                title: title.to_string(),
                original_content: original.clone(),
                contextualized_content: contextualized.clone(),
                source_url: source_url.map(String::from),
                doc_type: doc_type.to_string(),
                ingested_at: now.clone(),
                embedding,
            })
            .collect();

        let count = points.len();
        clients.qdrant.upsert_points(points).await?;
        inserted += count;
    }

    let _ = collection; // collection name is already set in QdrantClient config
    Ok(inserted)
}

/// Ingest documents from a URL by crawling.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_url(
    config: &AppConfig,
    entry_url: &str,
    depth: usize,
    collection: &str,
    chunk_size: usize,
    chunk_overlap: usize,
    batch_size: usize,
    _concurrency: usize,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let clients = IngestClients::from_config(config).await?;

    info!("Discovering URLs from {entry_url} with depth {depth}");
    let urls = crawl_urls(entry_url, depth).await?;
    info!("Discovered {} URLs", urls.len());

    let client = reqwest::Client::builder()
        .user_agent("Kenjaku-Ingester/0.1")
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let mut total_chunks = 0usize;
    let mut total_docs = 0usize;
    let mut total_errors = 0usize;

    for url in &urls {
        let doc_id = Uuid::new_v4().to_string();
        let result = client.get(url).send().await;

        match result {
            Ok(response) if response.status().is_success() => {
                let html = response.text().await.unwrap_or_default();
                let text = extract_text_from_html(&html);
                if text.trim().is_empty() {
                    warn!(url = %url, "Extracted text is empty, skipping");
                    continue;
                }
                let title = title_from_url(url);

                match process_document(
                    &clients,
                    &doc_id,
                    &title,
                    &text,
                    Some(url),
                    "html",
                    collection,
                    chunk_size,
                    chunk_overlap,
                    batch_size,
                )
                .await
                {
                    Ok(n) => {
                        total_chunks += n;
                        total_docs += 1;
                        info!(url = %url, chunks = n, "Ingested document");
                    }
                    Err(e) => {
                        error!(url = %url, error = %e, "Failed to process document");
                        total_errors += 1;
                    }
                }
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
        docs = total_docs,
        chunks = total_chunks,
        errors = total_errors,
        elapsed_secs = elapsed.as_secs(),
        "URL ingestion complete"
    );

    Ok(())
}

/// Ingest documents from a local folder.
pub async fn ingest_folder(
    config: &AppConfig,
    folder_path: &str,
    collection: &str,
    chunk_size: usize,
    chunk_overlap: usize,
    batch_size: usize,
    _concurrency: usize,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let clients = IngestClients::from_config(config).await?;

    // Canonicalize and validate the folder path.
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path
    let canonical_path = Path::new(folder_path)
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("Failed to resolve folder path {folder_path}: {e}"))?;

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

    let mut total_chunks = 0usize;
    let mut total_docs = 0usize;
    let mut total_errors = 0usize;

    for file_path in &files {
        let doc_id = Uuid::new_v4().to_string();
        match parse_file(file_path) {
            Ok((text, doc_type)) => {
                let title = extract_title(&text, file_path);
                if text.trim().is_empty() {
                    warn!(file = %file_path.display(), "Extracted text is empty, skipping");
                    continue;
                }

                match process_document(
                    &clients,
                    &doc_id,
                    &title,
                    &text,
                    Some(&file_path.display().to_string()),
                    &doc_type.to_string(),
                    collection,
                    chunk_size,
                    chunk_overlap,
                    batch_size,
                )
                .await
                {
                    Ok(n) => {
                        total_chunks += n;
                        total_docs += 1;
                        info!(
                            file = %file_path.display(),
                            doc_type = %doc_type,
                            chunks = n,
                            "Ingested document"
                        );
                    }
                    Err(e) => {
                        error!(file = %file_path.display(), error = %e, "Failed to process document");
                        total_errors += 1;
                    }
                }
            }
            Err(e) => {
                error!(file = %file_path.display(), error = %e, "Failed to parse");
                total_errors += 1;
            }
        }
    }

    let elapsed = start.elapsed();
    info!(
        docs = total_docs,
        chunks = total_chunks,
        errors = total_errors,
        elapsed_secs = elapsed.as_secs(),
        "Folder ingestion complete"
    );

    Ok(())
}
