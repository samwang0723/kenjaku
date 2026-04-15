//! Search pipeline strategies.
//!
//! Each submodule implements the [`kenjaku_core::traits::pipeline::SearchPipeline`]
//! trait with a different strategy. `SearchOrchestrator` holds an
//! `Arc<dyn SearchPipeline>` and delegates — see `../harness/mod.rs`.
//!
//! Currently only [`single_pass::SinglePassPipeline`] is implemented.
//! Planned additions (separate specs):
//! - `agentic` — tool-calling loop that may issue multiple LLM turns
//! - `cached` — short-TTL response cache in front of a downstream pipeline

pub mod single_pass;

pub use single_pass::SinglePassPipeline;
