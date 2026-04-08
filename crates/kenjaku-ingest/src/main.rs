use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing::{info, warn};

use kenjaku_core::config::load_config;
use kenjaku_infra::clustering::LinfaClusterer;
use kenjaku_infra::llm::GeminiProvider;
use kenjaku_infra::postgres::{
    DefaultSuggestionsRepository, RefreshBatchesRepository, create_pool, run_migrations,
};
use kenjaku_infra::qdrant::QdrantClient;
use kenjaku_service::refresh_worker::SuggestionRefreshWorker;

pub mod chunker;
pub mod crawler;
pub mod parser;
pub mod pipeline;

#[derive(Parser)]
#[command(name = "kenjaku-ingest", about = "Document ingestion CLI for Kenjaku")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Crawl a URL and ingest discovered documents
    Url {
        /// Entry URL to start crawling
        #[arg(long)]
        entry: String,

        /// Maximum crawl depth
        #[arg(long, default_value = "2")]
        depth: usize,

        /// Qdrant collection name
        #[arg(long, default_value = "documents")]
        collection: String,

        /// Chunk size in TOKENS (not characters). 500-800 is a good default.
        #[arg(long, default_value = "800")]
        chunk_size: usize,

        /// Chunk overlap in TOKENS. ~10-15% of chunk_size is standard.
        #[arg(long, default_value = "100")]
        chunk_overlap: usize,

        /// Embedding batch size (OpenAI allows up to 2048 inputs per request)
        #[arg(long, default_value = "64")]
        batch_size: usize,

        /// Concurrent document processing
        #[arg(long, default_value = "4")]
        concurrency: usize,
    },
    /// Ingest documents from a local folder
    Folder {
        /// Path to the folder containing documents
        #[arg(long)]
        path: String,

        /// Qdrant collection name
        #[arg(long, default_value = "documents")]
        collection: String,

        /// Chunk size in TOKENS
        #[arg(long, default_value = "800")]
        chunk_size: usize,

        /// Chunk overlap in TOKENS
        #[arg(long, default_value = "100")]
        chunk_overlap: usize,

        /// Embedding batch size
        #[arg(long, default_value = "64")]
        batch_size: usize,

        /// Concurrent document processing
        #[arg(long, default_value = "4")]
        concurrency: usize,
    },
    /// Run the default-suggestions refresh pipeline once, immediately.
    ///
    /// Used for ops/manual rebuilds outside the daily 03:00 UTC schedule.
    /// `--force` rebuilds even if the corpus fingerprint is unchanged.
    /// `--dry-run` reports current refresh state without writing anything;
    /// it does not rebuild or recompute the corpus fingerprint.
    SeedRefreshNow {
        /// Force rebuild even if the corpus fingerprint is unchanged.
        #[arg(long)]
        force: bool,

        /// Report current refresh state only — no writes or rebuild.
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    info!("Kenjaku ingestion CLI starting");

    // Load config (reads base.yaml + {APP_ENV}.yaml + secrets.{APP_ENV}.yaml)
    let config = load_config()?;
    config.validate_secrets()?;

    match cli.command {
        Commands::Url {
            entry,
            depth,
            collection,
            chunk_size,
            chunk_overlap,
            batch_size,
            concurrency,
        } => {
            info!(url = %entry, depth = depth, chunk_size = chunk_size, "Starting URL ingestion");
            pipeline::ingest_url(
                &config,
                &entry,
                depth,
                &collection,
                chunk_size,
                chunk_overlap,
                batch_size,
                concurrency,
            )
            .await?;
        }
        Commands::SeedRefreshNow { force, dry_run } => {
            info!(force, dry_run, "Starting seed-refresh-now");
            run_seed_refresh_now(&config, force, dry_run).await?;
        }
        Commands::Folder {
            path,
            collection,
            chunk_size,
            chunk_overlap,
            batch_size,
            concurrency,
        } => {
            info!(path = %path, chunk_size = chunk_size, "Starting folder ingestion");
            pipeline::ingest_folder(
                &config,
                &path,
                &collection,
                chunk_size,
                chunk_overlap,
                batch_size,
                concurrency,
            )
            .await?;
        }
    }

    info!("Ingestion complete");
    Ok(())
}

/// Construct the suggestion refresh dependencies and run a single
/// refresh cycle. Used by the `seed-refresh-now` subcommand.
///
/// Dry-run mode short-circuits BEFORE acquiring the advisory lock or
/// touching the refresh_batches table — it only reports the currently
/// active batch (if any) and the qdrant collection size, so operators
/// can sanity-check what a real run would target. We deliberately do
/// NOT recompute the corpus fingerprint here because the helpers used
/// by `SuggestionRefreshWorker` are private to that module; honest
/// dry-run = "show me current state, don't write".
async fn run_seed_refresh_now(
    config: &kenjaku_core::config::AppConfig,
    force: bool,
    dry_run: bool,
) -> anyhow::Result<()> {
    let pg_pool = create_pool(&config.postgres).await?;
    run_migrations(&pg_pool).await?;

    let qdrant = QdrantClient::new(config.qdrant.clone()).await?;
    qdrant.ensure_collection().await?;

    let default_repo = DefaultSuggestionsRepository::new(pg_pool.clone());
    let refresh_repo = RefreshBatchesRepository::new(pg_pool.clone());

    if dry_run {
        let latest = refresh_repo.latest_active().await?;
        let points = qdrant
            .collection_info(&config.qdrant.collection_name)
            .await
            .unwrap_or(0);
        info!(
            collection = %config.qdrant.collection_name,
            points = points,
            latest_active = ?latest,
            "seed-refresh-now dry-run: no writes performed"
        );
        return Ok(());
    }

    let llm_provider = Arc::new(GeminiProvider::new(config.llm.clone()));
    let clusterer = Arc::new(LinfaClusterer::new());

    let worker = SuggestionRefreshWorker::new(
        pg_pool.clone(),
        Arc::new(qdrant),
        clusterer,
        llm_provider,
        default_repo,
        refresh_repo,
        config.default_suggestions.clone(),
        config.qdrant.collection_name.clone(),
    );

    match worker.run_once(force).await {
        Ok(summary) => {
            info!(?summary, "seed-refresh-now completed");
        }
        Err(e) => {
            warn!(error = %e, "seed-refresh-now failed");
            return Err(anyhow::anyhow!(e));
        }
    }
    Ok(())
}
