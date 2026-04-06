use clap::{Parser, Subcommand};
use tracing::info;

pub mod chunker;
pub mod crawler;
pub mod parser;
pub mod pipeline;

#[derive(Parser)]
#[command(name = "kenjaku-ingest", about = "Document ingestion CLI for Kenjaku")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to config directory
    #[arg(long, default_value = "config")]
    config: String,
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

        /// Chunk size in characters
        #[arg(long, default_value = "512")]
        chunk_size: usize,

        /// Chunk overlap in characters
        #[arg(long, default_value = "50")]
        chunk_overlap: usize,

        /// Embedding batch size
        #[arg(long, default_value = "100")]
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

        /// Chunk size in characters
        #[arg(long, default_value = "512")]
        chunk_size: usize,

        /// Chunk overlap in characters
        #[arg(long, default_value = "50")]
        chunk_overlap: usize,

        /// Embedding batch size
        #[arg(long, default_value = "100")]
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
            info!(url = %entry, depth = depth, "Starting URL ingestion");
            pipeline::ingest_url(
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
            info!(path = %path, "Starting folder ingestion");
            pipeline::ingest_folder(
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
