pub mod openai;

pub use self::openai::OpenAiEmbeddingProvider;

use kenjaku_core::config::EmbeddingConfig;
use kenjaku_core::error::{Error, Result};
use kenjaku_core::traits::embedding::EmbeddingProvider;

/// Factory for creating embedding providers based on configuration.
pub fn create_embedding_provider(config: EmbeddingConfig) -> Result<Box<dyn EmbeddingProvider>> {
    match config.provider.as_str() {
        "openai" => Ok(Box::new(OpenAiEmbeddingProvider::new(config))),
        other => Err(Error::Config(format!(
            "Unknown embedding provider: {other}. Supported: openai"
        ))),
    }
}
