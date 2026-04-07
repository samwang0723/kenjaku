use clap::{Parser, Subcommand};
use tracing::info;

use kenjaku_core::config::load_config;

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
