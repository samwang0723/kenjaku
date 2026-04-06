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

impl Error {
    /// Returns a safe, user-facing error message that does not leak internals.
    /// Use this in API handlers instead of `e.to_string()`.
    pub fn user_message(&self) -> &str {
        match self {
            Error::Validation(msg) => msg,
            Error::NotFound(msg) => msg,
            Error::Config(_) => "Service configuration error",
            Error::Embedding(_) => "Embedding service unavailable",
            Error::Llm(_) => "LLM service unavailable",
            Error::VectorStore(_) => "Search service unavailable",
            Error::Database(_) => "Database service unavailable",
            Error::Cache(_) => "Cache service unavailable",
            Error::Ingestion(_) => "Ingestion error",
            Error::Translation(_) => "Translation service unavailable",
            Error::ExternalService { .. } => "External service unavailable",
            Error::Internal(_) => "Internal server error",
        }
    }
}

/// Result type alias for Kenjaku operations.
pub type Result<T> = std::result::Result<T, Error>;
