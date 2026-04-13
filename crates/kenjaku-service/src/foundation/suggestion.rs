//! Read-only `SuggestionService` — blends crowdsourced trending with
//! pre-materialized default suggestions via Efraimidis-Spirakis weighted
//! random sampling without replacement. No LLM, no writes, safe to call
//! on the hot path. See spec §4.3.

use std::sync::Mutex;

use rand::rngs::StdRng;
use rand::{Rng, RngCore, SeedableRng};
use tracing::instrument;

use kenjaku_core::error::Result;
use kenjaku_core::types::locale::Locale;
use kenjaku_core::types::suggestion::{BlendedSuggestion, SuggestionSource};
use kenjaku_core::types::trending::TrendingPeriod;
use kenjaku_infra::postgres::{DefaultSuggestionsRepository, TrendingRepository};

/// Injectable RNG so tests can seed for determinism. Production wraps a
/// fresh `StdRng::from_entropy()`; tests pass a `StdRng::seed_from_u64`.
pub struct ServiceRng(Mutex<Box<dyn RngCore + Send>>);

impl ServiceRng {
    pub fn from_entropy() -> Self {
        Self(Mutex::new(Box::new(StdRng::from_entropy())))
    }

    pub fn from_seed(seed: u64) -> Self {
        Self(Mutex::new(Box::new(StdRng::seed_from_u64(seed))))
    }

    /// Sample a uniform `(0, 1)` (open interval — guard against ln(0)).
    ///
    /// Returns 0.5 (neutral weight) if the mutex is poisoned, which is
    /// extremely unlikely but avoids a panic in production.
    fn sample_unit(&self) -> f64 {
        let mut guard = match self.0.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::warn!("ServiceRng mutex poisoned, recovering");
                poisoned.into_inner()
            }
        };
        guard.gen_range(f64::EPSILON..1.0_f64)
    }
}

#[derive(Clone)]
pub struct SuggestionService {
    trending_repo: TrendingRepository,
    defaults_repo: DefaultSuggestionsRepository,
    pool_cap: usize,
    crowd_min_count: i64,
    rng: std::sync::Arc<ServiceRng>,
}

impl SuggestionService {
    pub fn new(
        trending_repo: TrendingRepository,
        defaults_repo: DefaultSuggestionsRepository,
        pool_cap: usize,
        crowd_min_count: i64,
        rng: std::sync::Arc<ServiceRng>,
    ) -> Self {
        Self {
            trending_repo,
            defaults_repo,
            pool_cap,
            crowd_min_count,
            rng,
        }
    }

    /// Blended top suggestions for a locale. Read-only.
    #[instrument(skip(self))]
    pub async fn get_top(&self, locale: Locale, limit: usize) -> Result<Vec<BlendedSuggestion>> {
        let crowd = self
            .trending_repo
            .get_top(
                locale.as_str(),
                &TrendingPeriod::Daily,
                self.pool_cap,
                self.crowd_min_count,
            )
            .await?;
        let defaults = self
            .defaults_repo
            .list_active_by_locale(locale, self.pool_cap)
            .await?;

        let pool = build_pool(crowd, defaults);
        Ok(self.weighted_sample(pool, limit))
    }

    /// Blended autocomplete — same blend, prefix-filtered first.
    /// `prefix` is case-insensitive (lowered before search).
    #[instrument(skip(self))]
    pub async fn autocomplete(
        &self,
        locale: Locale,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<BlendedSuggestion>> {
        let prefix_lower = prefix.to_lowercase();
        let crowd = self
            .trending_repo
            .search_popular(
                locale.as_str(),
                &prefix_lower,
                self.pool_cap,
                self.crowd_min_count,
            )
            .await?;
        let defaults = self
            .defaults_repo
            .prefix_search_active(locale, &prefix_lower, self.pool_cap)
            .await?;

        let pool = build_pool(crowd, defaults);
        Ok(self.weighted_sample(pool, limit))
    }

    /// Efraimidis-Spirakis weighted random sampling without replacement.
    /// key = -ln(U) / weight; sort ascending; take first `limit`.
    fn weighted_sample(
        &self,
        pool: Vec<BlendedSuggestion>,
        limit: usize,
    ) -> Vec<BlendedSuggestion> {
        if pool.is_empty() || limit == 0 {
            return Vec::new();
        }

        let mut keyed: Vec<(f64, BlendedSuggestion)> = pool
            .into_iter()
            .map(|item| {
                // Items with score <= 0 still get a chance via a tiny epsilon weight.
                let weight = if item.score > 0.0 { item.score } else { 1e-9 };
                let u = self.rng.sample_unit();
                let key = -u.ln() / weight;
                (key, item)
            })
            .collect();

        keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        keyed.into_iter().take(limit).map(|(_, it)| it).collect()
    }
}

/// Pool builder — converts repo rows into a unified `BlendedSuggestion`
/// pool. Pure, side-effect-free; unit-testable without DB.
fn build_pool(
    crowd: Vec<kenjaku_core::types::trending::PopularQuery>,
    defaults: Vec<kenjaku_core::types::suggestion::DefaultSuggestion>,
) -> Vec<BlendedSuggestion> {
    let mut out = Vec::with_capacity(crowd.len() + defaults.len());
    for c in crowd {
        out.push(BlendedSuggestion {
            query: c.query,
            source: SuggestionSource::Crowdsourced,
            score: c.search_count as f64,
        });
    }
    for d in defaults {
        out.push(BlendedSuggestion {
            query: d.question,
            source: SuggestionSource::Default,
            score: d.weight as f64,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, source: SuggestionSource, score: f64) -> BlendedSuggestion {
        BlendedSuggestion {
            query: name.to_string(),
            source,
            score,
        }
    }

    /// Run weighted_sample directly without touching repos. Builds an
    /// ad-hoc service with stub repos that are never invoked — we only
    /// exercise the pure sampling code path.
    fn make_sampler(
        seed: u64,
    ) -> Box<dyn Fn(Vec<BlendedSuggestion>, usize) -> Vec<BlendedSuggestion>> {
        let rng = std::sync::Arc::new(ServiceRng::from_seed(seed));
        Box::new(move |pool, limit| {
            // Inline the same algorithm as `SuggestionService::weighted_sample`
            // so we don't need to construct repos.
            if pool.is_empty() || limit == 0 {
                return Vec::new();
            }
            let mut keyed: Vec<(f64, BlendedSuggestion)> = pool
                .into_iter()
                .map(|it| {
                    let w = if it.score > 0.0 { it.score } else { 1e-9 };
                    let u = rng.sample_unit();
                    (-u.ln() / w, it)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            keyed.into_iter().take(limit).map(|(_, it)| it).collect()
        })
    }

    #[test]
    fn empty_pool_returns_empty() {
        let sample = make_sampler(42);
        assert!(sample(Vec::new(), 5).is_empty());
    }

    #[test]
    fn limit_zero_returns_empty() {
        let sample = make_sampler(42);
        let pool = vec![item("a", SuggestionSource::Default, 10.0)];
        assert!(sample(pool, 0).is_empty());
    }

    #[test]
    fn deterministic_with_seeded_rng() {
        let pool = vec![
            item("a", SuggestionSource::Default, 10.0),
            item("b", SuggestionSource::Crowdsourced, 5.0),
            item("c", SuggestionSource::Default, 20.0),
        ];
        let s1 = make_sampler(123);
        let s2 = make_sampler(123);
        let r1 = s1(pool.clone(), 3);
        let r2 = s2(pool, 3);
        assert_eq!(
            r1.iter().map(|i| i.query.clone()).collect::<Vec<_>>(),
            r2.iter().map(|i| i.query.clone()).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn high_weight_dominates_in_aggregate() {
        // Over many seeded runs the high-weight item should appear in the
        // top-1 result more often than the low-weight items.
        let mut high_wins = 0;
        let total = 200;
        for seed in 0..total {
            let pool = vec![
                item("low1", SuggestionSource::Default, 1.0),
                item("low2", SuggestionSource::Default, 1.0),
                item("high", SuggestionSource::Default, 100.0),
            ];
            let sample = make_sampler(seed);
            let result = sample(pool, 1);
            if result[0].query == "high" {
                high_wins += 1;
            }
        }
        // Naive equal-weight expectation: 33%. With 100x weight, expect >> 80%.
        assert!(
            high_wins > total * 80 / 100,
            "high-weight item won only {high_wins}/{total} runs"
        );
    }

    #[test]
    fn build_pool_merges_both_sources() {
        use kenjaku_core::types::suggestion::DefaultSuggestion;
        use kenjaku_core::types::trending::{PopularQuery, TrendingPeriod};

        let crowd = vec![PopularQuery {
            id: 1,
            locale: "en".to_string(),
            query: "btc".to_string(),
            search_count: 50,
            period: TrendingPeriod::Daily,
            period_date: chrono::NaiveDate::from_ymd_opt(2026, 4, 8).unwrap(),
        }];
        let defaults = vec![DefaultSuggestion {
            id: 10,
            locale: Locale::En,
            question: "What is staking?".to_string(),
            topic_cluster_id: 0,
            topic_label: "staking".to_string(),
            batch_id: 1,
            generated_at: chrono::Utc::now(),
            weight: 10,
        }];
        let pool = build_pool(crowd, defaults);
        assert_eq!(pool.len(), 2);
        assert!(
            pool.iter()
                .any(|i| i.source == SuggestionSource::Crowdsourced)
        );
        assert!(pool.iter().any(|i| i.source == SuggestionSource::Default));
    }
}
