//! Authentication primitives.
//!
//! Phase 3c.1 introduces the JWT validator. Slice 3c.2 adds the Axum
//! middleware that invokes it, the tenants cache, and the per-tenant
//! rate-limit extractor. Everything here is a pure library — no
//! middleware, no HTTP surface.

pub mod jwt;

pub use jwt::{JwtValidator, TenantClaims};
