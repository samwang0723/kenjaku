use thiserror::Error;

/// Domain-level errors for the Kenjaku system.
#[derive(Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("Vector store error: {0}")]
    VectorStore(String),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("Ingestion error: {0}")]
    Ingestion(String),

    #[error("Translation error: {0}")]
    Translation(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("External service error: {source}")]
    ExternalService {
        service: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Result type alias for Kenjaku operations.
pub type Result<T> = std::result::Result<T, Error>;
