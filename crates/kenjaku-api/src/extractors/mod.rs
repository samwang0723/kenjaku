//! Axum extractors specific to the kenjaku API surface.

pub mod locale;

pub use locale::{ResolvedLocale, ResolvedLocaleSource, SessionLocaleLookup};
