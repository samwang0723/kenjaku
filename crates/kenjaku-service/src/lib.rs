pub mod brain;
pub mod foundation;
pub(crate) mod harness;
pub mod search;
pub mod session;
pub mod tools;

// Re-exports for backward compatibility — external crates import these.
// Gradually deprecate as callers migrate to the layered paths.
pub use brain::intent;
pub use brain::translation;
pub use foundation::quality;
pub use foundation::suggestion;
pub use foundation::trending;
pub use foundation::worker;
pub use foundation::worker::suggestion as refresh_worker;
pub use harness::component;
pub use session::autocomplete;
pub use session::conversation;
pub use session::feedback;
pub use session::history;
pub use session::locale_memory;
pub use tools::reranker;
pub use tools::retriever;
