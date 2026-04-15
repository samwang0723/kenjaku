//! Tenants cache (Phase 3c.2).
//!
//! Startup-loaded, read-only cache of the `tenants` table seeded in
//! Phase 3a. The auth middleware uses [`TenantsCache::get`] to map a
//! JWT's `tenant_id` claim into a concrete DB row whose `plan_tier`
//! is the **authoritative** source for authorization decisions — the
//! JWT claim's `plan_tier` field is advisory only (see
//! `kenjaku_infra::auth::jwt` module docs).
//!
//! # Cache semantics
//!
//! - Loaded once at startup. No refresh.
//! - Read-only after construction: the internal map lives behind an
//!   `Arc<HashMap<...>>` (NOT `Arc<RwLock<_>>`) so the
//!   "no writes after startup" invariant is structurally enforced by
//!   the compiler. Swap to `Arc<RwLock<_>>` when refresh lands in 3d
//!   or later.
//! - Adding / suspending a tenant requires a container restart. For
//!   Crypto.com's whitelabel-B2B pace this is acceptable (tenants
//!   onboard on a weeks-not-hours cadence) per the 3c architect
//!   decision.
//!
//! # Logging
//!
//! [`TenantRow`] has a **custom `Debug` impl** that redacts
//! `config_overrides`. The `config_overrides: serde_json::Value`
//! field may contain per-tenant settings that MUST NOT appear in log
//! lines. Never derive `Debug` on this struct.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value as JsonValue;
use sqlx::{PgPool, Row};
use tracing::instrument;

use kenjaku_core::error::{Error, Result};
use kenjaku_core::types::tenant::{PlanTier, TenantId};

/// A single row from the `tenants` table.
///
/// `config_overrides` is a raw JSONB value; readers MUST use the
/// typed accessors (3c.2 has no per-tenant override readers yet —
/// all reads go through the plan-tier default paths in the rate
/// limiter). Direct field access is still `pub` for future
/// accessor impls, but the redacted `Debug` prevents the field from
/// ever being accidentally logged.
///
/// `TenantRow` deliberately does **NOT** implement [`Clone`]. Shared
/// access goes through [`Arc<TenantRow>`] ([`TenantsCache::get`]
/// returns one), so cloning the full JSONB in `config_overrides` is
/// structurally impossible on the request hot path.
pub struct TenantRow {
    pub id: TenantId,
    pub name: String,
    pub plan_tier: PlanTier,
    /// Per-tenant JSONB overrides. **NEVER logged.** Redacted by the
    /// custom `Debug` impl below.
    pub config_overrides: JsonValue,
}

impl std::fmt::Debug for TenantRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately redacts `config_overrides`. See module docs.
        f.debug_struct("TenantRow")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("plan_tier", &self.plan_tier)
            .field("config_overrides", &"<redacted>")
            .finish()
    }
}

/// Repository abstraction over the `tenants` table.
#[derive(Clone)]
pub struct TenantRepository {
    pool: PgPool,
}

impl TenantRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Fetch every row. Used once at startup to populate
    /// [`TenantsCache`].
    ///
    /// Explicit column list — no `SELECT *` — so a future schema
    /// addition doesn't silently bleed new fields into the
    /// in-memory cache.
    ///
    /// Returns `HashMap<TenantId, Arc<TenantRow>>`. The Arc wrapper
    /// lets the cache hand out cheap (refcount-bump) handles on the
    /// request hot path instead of cloning the full JSONB
    /// `config_overrides` on every auth check.
    #[instrument(skip(self))]
    pub async fn load_all(&self) -> Result<HashMap<TenantId, Arc<TenantRow>>> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, plan_tier, config_overrides
            FROM tenants
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Database(format!("load_all tenants failed: {e}")))?;

        let mut out = HashMap::with_capacity(rows.len());
        for row in rows {
            let id_raw: String = row
                .try_get("id")
                .map_err(|e| Error::Database(format!("tenants.id read: {e}")))?;
            let id = TenantId::new(&id_raw).map_err(|e| {
                // Any id that fails validation here is a data-quality
                // bug — row was inserted before 3a's charset rules.
                // Fail startup rather than ignore; a bad id must not
                // end up in the cache.
                Error::Database(format!(
                    "invalid tenant id {id_raw:?} in tenants table: {e}"
                ))
            })?;
            let name: String = row
                .try_get("name")
                .map_err(|e| Error::Database(format!("tenants.name read: {e}")))?;
            let plan_tier_raw: String = row
                .try_get("plan_tier")
                .map_err(|e| Error::Database(format!("tenants.plan_tier read: {e}")))?;
            let plan_tier = parse_plan_tier(&plan_tier_raw)?;
            let config_overrides: JsonValue = row
                .try_get("config_overrides")
                .map_err(|e| Error::Database(format!("tenants.config_overrides read: {e}")))?;
            out.insert(
                id.clone(),
                Arc::new(TenantRow {
                    id,
                    name,
                    plan_tier,
                    config_overrides,
                }),
            );
        }
        Ok(out)
    }
}

/// Parse the DB `plan_tier` TEXT column (constrained to
/// `'free' | 'pro' | 'enterprise'` at the column level) into the
/// typed enum.
///
/// `PlanTier::Suspended` is deliberately not part of the enum yet —
/// the 3c.2 dispatch defers that (and its accompanying CHECK
/// constraint expansion) to 3d. A future row value of `'suspended'`
/// will fail this parse, which is the right startup behavior: a
/// deploy that accidentally includes a row ahead of the code change
/// fails fast rather than silently misclassifying.
fn parse_plan_tier(s: &str) -> Result<PlanTier> {
    match s {
        "free" => Ok(PlanTier::Free),
        "pro" => Ok(PlanTier::Pro),
        "enterprise" => Ok(PlanTier::Enterprise),
        other => Err(Error::Database(format!(
            "unknown plan_tier {other:?} in tenants table"
        ))),
    }
}

/// In-memory tenants cache.
///
/// Constructed once at startup via [`TenantsCache::load_at_startup`].
/// Thread-safe: the internal map lives behind an `Arc<HashMap>` with
/// no interior mutability — all callers share the same read-only
/// snapshot. Cheap to `clone()` (just an Arc bump).
///
/// Rows are stored as `Arc<TenantRow>` so [`TenantsCache::get`] can
/// hand out refcount-bump handles without cloning the row's JSONB
/// `config_overrides` on every auth check.
#[derive(Clone)]
pub struct TenantsCache {
    tenants: Arc<HashMap<TenantId, Arc<TenantRow>>>,
}

impl TenantsCache {
    /// Load every row from the `tenants` table into memory.
    ///
    /// Called from the server binary's `main.rs` immediately after
    /// migrations run. The `public` row seeded by migration
    /// `20260415000001_add_tenant_id.up.sql` is always present.
    pub async fn load_at_startup(pool: &PgPool) -> Result<Self> {
        let repo = TenantRepository::new(pool.clone());
        let map = repo.load_all().await?;
        Ok(Self::from_map(map))
    }

    /// Construct from a pre-built map. Primarily for tests.
    pub fn from_map(map: HashMap<TenantId, Arc<TenantRow>>) -> Self {
        Self {
            tenants: Arc::new(map),
        }
    }

    /// Look up a tenant by id. Cheap (`HashMap::get` + `Arc::clone`).
    ///
    /// Returns `Option<Arc<TenantRow>>` instead of `Option<TenantRow>`
    /// — the Arc clone is a single atomic refcount bump, while a
    /// plain `TenantRow` clone would copy the full JSONB
    /// `config_overrides` on every authenticated request.
    pub fn get(&self, id: &TenantId) -> Option<Arc<TenantRow>> {
        self.tenants.get(id).cloned()
    }

    /// Number of tenants loaded. Useful for startup audit logs.
    pub fn len(&self) -> usize {
        self.tenants.len()
    }

    /// Whether the cache is empty. Should never be `true` in practice
    /// because the `public` row is seeded.
    pub fn is_empty(&self) -> bool {
        self.tenants.is_empty()
    }
}

impl std::fmt::Debug for TenantsCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantsCache")
            .field("len", &self.tenants.len())
            .finish_non_exhaustive()
    }
}

// =====================================================================
// Tests (unit — no live PG)
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    fn row(id: &str, plan: PlanTier) -> TenantRow {
        TenantRow {
            id: TenantId::new(id).unwrap(),
            name: format!("{id} Corp"),
            plan_tier: plan,
            config_overrides: json!({"rate_limit": {"per_second": 42}}),
        }
    }

    fn populated_cache() -> TenantsCache {
        let mut m = HashMap::new();
        for (id, plan) in [
            ("public", PlanTier::Enterprise),
            ("acme", PlanTier::Pro),
            ("beta-co", PlanTier::Free),
        ] {
            let r = row(id, plan);
            // Arc wrap matches production shape: cache rows are
            // `Arc<TenantRow>` so reads don't clone JSONB.
            m.insert(r.id.clone(), Arc::new(r));
        }
        TenantsCache::from_map(m)
    }

    #[test]
    fn empty_cache_len_is_zero() {
        let c = TenantsCache::from_map(HashMap::new());
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);
    }

    #[test]
    fn populated_cache_reports_len() {
        let c = populated_cache();
        assert!(!c.is_empty());
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn get_hit_returns_row_with_expected_plan_tier() {
        let c = populated_cache();
        let acme = TenantId::new("acme").unwrap();
        let r: Arc<TenantRow> = c.get(&acme).expect("acme must be present");
        // Arc derefs transparently to TenantRow's fields.
        assert_eq!(r.id.as_str(), "acme");
        assert_eq!(r.plan_tier, PlanTier::Pro);
    }

    #[test]
    fn get_returns_arc_not_full_clone() {
        // Regression guard for PR #17 #3: `get` must hand out an
        // `Arc<TenantRow>` so the request hot path doesn't clone the
        // JSONB `config_overrides` on every authenticated request.
        // Two independent `get` calls for the same key must share
        // the same heap allocation — `Arc::ptr_eq` is true and the
        // strong count grows by the number of live handles.
        let c = populated_cache();
        let acme = TenantId::new("acme").unwrap();
        let a = c.get(&acme).unwrap();
        let b = c.get(&acme).unwrap();
        assert!(
            Arc::ptr_eq(&a, &b),
            "two gets on the same key must return the same Arc allocation"
        );
        // Cache-internal strong count (1) + `a` + `b` = 3.
        assert_eq!(Arc::strong_count(&a), 3);
    }

    #[test]
    fn get_miss_returns_none() {
        let c = populated_cache();
        let nope = TenantId::new("nope").unwrap();
        assert!(c.get(&nope).is_none());
    }

    #[test]
    fn plan_tier_variants_roundtrip_through_parse_plan_tier() {
        assert_eq!(parse_plan_tier("free").unwrap(), PlanTier::Free);
        assert_eq!(parse_plan_tier("pro").unwrap(), PlanTier::Pro);
        assert_eq!(parse_plan_tier("enterprise").unwrap(), PlanTier::Enterprise);
    }

    #[test]
    fn parse_plan_tier_rejects_unknown() {
        // Future `suspended` value from a pre-deploy of 3d must fail
        // startup here — better than silently misclassifying.
        for bad in ["suspended", "Free", "", "pro-plus", "PRO"] {
            assert!(
                parse_plan_tier(bad).is_err(),
                "parse_plan_tier must reject {bad:?}"
            );
        }
    }

    #[test]
    fn tenant_row_debug_redacts_config_overrides() {
        let r = row("acme", PlanTier::Pro);
        let dbg = format!("{r:?}");
        // Name + id + plan_tier appear; config_overrides does NOT.
        assert!(dbg.contains("acme"), "debug must include id: {dbg}");
        assert!(dbg.contains("Pro"), "debug must include plan_tier: {dbg}");
        assert!(
            dbg.contains("<redacted>"),
            "debug must redact config_overrides: {dbg}"
        );
        assert!(
            !dbg.contains("per_second"),
            "debug must NOT leak inner JSONB key: {dbg}"
        );
        assert!(
            !dbg.contains("42"),
            "debug must NOT leak inner JSONB value: {dbg}"
        );
    }

    #[test]
    fn tenants_cache_debug_redacts_inner_rows() {
        // Even the outer cache's Debug must avoid listing rows —
        // logging `{:?}` of the cache should only surface counts.
        let c = populated_cache();
        let dbg = format!("{c:?}");
        assert!(
            !dbg.contains("per_second"),
            "cache debug leaks JSONB: {dbg}"
        );
        assert!(!dbg.contains("Corp"), "cache debug leaks name field: {dbg}");
        assert!(dbg.contains("TenantsCache"));
    }
}
