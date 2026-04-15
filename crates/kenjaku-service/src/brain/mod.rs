//! Brain facade — LLM-agnostic abstraction over prompt construction,
//! conversation assembly, and model invocation.
//!
//! Public exports:
//! - `CompositeBrain` — Phase 2 orchestrator-facing `Brain` impl; composes
//!   `Classifier` + `Translator` + `Generator` sub-traits
//! - `GeminiBrain` — the default underlying provider-backed `Brain` impl
//!   used by `CompositeBrain` (serves all three sub-roles today)
//! - `ConversationAssembler` — pure function: history + query + chunks -> Vec<Message>
//! - Prompt builders in `prompt` submodule

pub mod assembler;
pub mod composite;
pub mod generator;
pub mod intent;
pub mod prompt;
pub mod translation;

pub use assembler::ConversationAssembler;
pub use composite::CompositeBrain;
pub use generator::GeminiBrain;
