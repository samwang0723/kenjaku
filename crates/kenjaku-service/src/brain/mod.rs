//! Brain facade — LLM-agnostic abstraction over prompt construction,
//! conversation assembly, and model invocation.
//!
//! Public exports:
//! - `GeminiBrain` — the default `Brain` impl wrapping `Arc<dyn LlmProvider>`
//! - `ConversationAssembler` — pure function: history + query + chunks -> Vec<Message>
//! - Prompt builders in `prompt` submodule

pub mod assembler;
pub mod generator;
pub mod intent;
pub mod prompt;
pub mod translation;

pub use assembler::ConversationAssembler;
pub use generator::GeminiBrain;
