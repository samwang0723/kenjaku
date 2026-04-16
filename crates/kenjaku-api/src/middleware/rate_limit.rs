//! Tenant-aware rate-limit key extractor (Phase 3c.2, updated 3e).
//!
//! Plugs into `tower_governor::governor::GovernorConfigBuilder::key_extractor`.
//! The strategy (`Ip` / `TenantIp` / `TenantPrincipalIp`) is chosen at
//! startup from `config.rate_limit.key_strategy`.
//!
//! **`Ip` strategy delegates to the existing `SmartIpKeyExtractor`** —
//! pure IP-based rate limiting.
//!
//! **Tenant-aware strategies read `TenantContext` from request
//! extensions**, placed by the auth middleware (always on in Phase 3e).

use std::net::IpAddr;

use tower_governor::errors::GovernorError;
use tower_governor::key_extractor::{KeyExtractor, SmartIpKeyExtractor};

use kenjaku_core::config::RateLimitKeyStrategy;
use kenjaku_core::types::tenant::TenantContext;

/// Composite key shapes. Stored as a 3-arm enum so
/// `tower_governor`'s `type Key: Hash + Eq + Debug` bound is satisfied
/// with a single concrete type regardless of strategy.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum RateLimitKey {
    Ip(IpAddr),
    TenantIp {
        tenant_id: String,
        ip: IpAddr,
    },
    TenantPrincipalIp {
        tenant_id: String,
        // Principal id if present; else the stringified IP so
        // unauthenticated / no-principal traffic still gets a stable
        // bucket rather than all collapsing into one tenant-wide key.
        principal_or_ip: String,
        ip: IpAddr,
    },
}

/// The actual `KeyExtractor` wired into `tower_governor`.
///
/// Cheap to clone — just copies the strategy enum.
#[derive(Debug, Clone, Copy)]
pub struct TenantPrincipalIpExtractor {
    strategy: RateLimitKeyStrategy,
}

impl TenantPrincipalIpExtractor {
    pub fn new(strategy: RateLimitKeyStrategy) -> Self {
        Self { strategy }
    }
}

impl KeyExtractor for TenantPrincipalIpExtractor {
    type Key = RateLimitKey;

    fn extract<T>(&self, req: &http::Request<T>) -> Result<Self::Key, GovernorError> {
        // Always resolve the IP via the existing SmartIpKeyExtractor
        // (X-Forwarded-For → X-Real-Ip → Forwarded → peer). If no IP
        // at all, bubble up `UnableToExtractKey`.
        let ip = SmartIpKeyExtractor.extract(req)?;

        Ok(match self.strategy {
            RateLimitKeyStrategy::Ip => RateLimitKey::Ip(ip),
            RateLimitKeyStrategy::TenantIp => {
                let tenant_id = tenant_id_or_public(req);
                RateLimitKey::TenantIp { tenant_id, ip }
            }
            RateLimitKeyStrategy::TenantPrincipalIp => {
                let tenant_id = tenant_id_or_public(req);
                let principal_or_ip = principal_id_or_ip(req, ip);
                RateLimitKey::TenantPrincipalIp {
                    tenant_id,
                    principal_or_ip,
                    ip,
                }
            }
        })
    }
}

/// Read `TenantContext.tenant_id` from request extensions. Falls back
/// to `"public"` when the auth middleware didn't run (e.g. route
/// outside the global layer stack) — safer than panicking mid-request.
fn tenant_id_or_public<T>(req: &http::Request<T>) -> String {
    req.extensions()
        .get::<TenantContext>()
        .map(|t| t.tenant_id.as_str().to_string())
        .unwrap_or_else(|| "public".to_string())
}

/// Read `TenantContext.principal_id` from extensions; fall back to
/// the IP string if absent. Guarantees a stable non-empty bucket.
fn principal_id_or_ip<T>(req: &http::Request<T>, ip: IpAddr) -> String {
    req.extensions()
        .get::<TenantContext>()
        .and_then(|t| t.principal_id.as_ref())
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| ip.to_string())
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request;

    use kenjaku_core::types::tenant::{PlanTier, PrincipalId, TenantContext, TenantId};

    fn req_with_ctx_and_ip(tctx: Option<TenantContext>) -> Request<Body> {
        let mut req = Request::builder()
            .header("x-forwarded-for", "203.0.113.7")
            .body(Body::empty())
            .unwrap();
        // tower_governor reads ConnectInfo as a fallback; we supply
        // it too for robustness.
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
        if let Some(c) = tctx {
            req.extensions_mut().insert(c);
        }
        req
    }

    fn tctx(tenant: &str, principal: Option<&str>, plan: PlanTier) -> TenantContext {
        TenantContext {
            tenant_id: TenantId::new(tenant).unwrap(),
            principal_id: principal.map(|p| PrincipalId::new(p).unwrap()),
            plan_tier: plan,
        }
    }

    // Ipv4Addr that matches what SmartIpKeyExtractor will parse out
    // of `X-Forwarded-For: 203.0.113.7`.
    fn expected_ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))
    }

    #[test]
    fn strategy_ip_produces_plain_ip_key() {
        let ex = TenantPrincipalIpExtractor::new(RateLimitKeyStrategy::Ip);
        // TenantContext present but should be ignored under `Ip`.
        let req = req_with_ctx_and_ip(Some(tctx("acme", Some("u1"), PlanTier::Pro)));
        let key = ex.extract(&req).unwrap();
        assert_eq!(key, RateLimitKey::Ip(expected_ip()));
    }

    #[test]
    fn strategy_tenant_ip_produces_tuple_key() {
        let ex = TenantPrincipalIpExtractor::new(RateLimitKeyStrategy::TenantIp);
        let req = req_with_ctx_and_ip(Some(tctx("acme", Some("u1"), PlanTier::Pro)));
        let key = ex.extract(&req).unwrap();
        assert_eq!(
            key,
            RateLimitKey::TenantIp {
                tenant_id: "acme".to_string(),
                ip: expected_ip(),
            }
        );
    }

    #[test]
    fn strategy_tenant_principal_ip_produces_triple_key() {
        let ex = TenantPrincipalIpExtractor::new(RateLimitKeyStrategy::TenantPrincipalIp);
        let req = req_with_ctx_and_ip(Some(tctx("acme", Some("u1"), PlanTier::Pro)));
        let key = ex.extract(&req).unwrap();
        assert_eq!(
            key,
            RateLimitKey::TenantPrincipalIp {
                tenant_id: "acme".to_string(),
                principal_or_ip: "u1".to_string(),
                ip: expected_ip(),
            }
        );
    }

    #[test]
    fn tenant_principal_ip_falls_back_to_ip_when_no_principal() {
        let ex = TenantPrincipalIpExtractor::new(RateLimitKeyStrategy::TenantPrincipalIp);
        // No principal (common when auth middleware placed `public`).
        let req = req_with_ctx_and_ip(Some(tctx("public", None, PlanTier::Enterprise)));
        let key = ex.extract(&req).unwrap();
        assert_eq!(
            key,
            RateLimitKey::TenantPrincipalIp {
                tenant_id: "public".to_string(),
                principal_or_ip: "203.0.113.7".to_string(),
                ip: expected_ip(),
            }
        );
    }

    #[test]
    fn tenant_strategies_fall_back_to_public_when_no_tctx_extension() {
        // Simulates a route that somehow slipped outside the auth
        // middleware stack. Expected safe fallback: public tenant.
        let ex = TenantPrincipalIpExtractor::new(RateLimitKeyStrategy::TenantIp);
        let req = req_with_ctx_and_ip(None);
        let key = ex.extract(&req).unwrap();
        assert_eq!(
            key,
            RateLimitKey::TenantIp {
                tenant_id: "public".to_string(),
                ip: expected_ip(),
            }
        );
    }
}
