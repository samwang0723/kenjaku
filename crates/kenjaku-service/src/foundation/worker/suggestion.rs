//! `SuggestionRefreshWorker` — the batch worker that rebuilds the
//! `default_suggestions` pool.
//!
//! Procedure (spec §4.4):
//! 1. Acquire a session-scoped `pg_try_advisory_lock` on a pinned
//!    connection. Bail if another replica already holds it.
//! 2. Scroll the Qdrant collection with vectors + payload up to
//!    `sample_cap`. Compute a stable fingerprint over `(collection,
//!    points_count, sorted_first_N_ids)` and skip the run when the
//!    fingerprint hasn't changed since the last active batch (unless
//!    `force = true`).
//! 3. Start a `running` refresh batch, k-means cluster the sample, and
//!    for each cluster pick the 5 member texts closest to the centroid
//!    to prompt the LLM. Questions returned per locale are safety-
//!    filtered and deduped.
//! 4. Bulk-insert the surviving rows, atomically swap the new batch to
//!    `active`, cascade-retain the most recent N batches, and release
//!    the advisory lock.
//!
//! Any error between `start_batch` and `swap_active_atomic` marks the
//! batch `failed` and releases the lock without touching the prior
//! active batch — reads keep serving stale data.

use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, TimeZone, Utc};
use cron::Schedule;
use regex::Regex;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::str::FromStr;
use tokio::time;
use tracing::{error, info, warn};

use kenjaku_core::config::{DefaultSuggestionsConfig, RefreshConfig};
use kenjaku_core::error::{Error, Result};
use kenjaku_core::traits::clusterer::{Cluster, Clusterer};
use kenjaku_core::traits::llm::LlmProvider;
use kenjaku_infra::postgres::{
    DefaultSuggestionsRepository, NewDefaultSuggestion, RefreshBatchesRepository,
};
use kenjaku_infra::qdrant::{QdrantClient, ScrolledPoint};

/// Outcome of a single `run_once` call. Used by tests and `/admin`
/// endpoints to report what happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunSummary {
    /// Another replica held the advisory lock.
    LockHeld,
    /// Corpus fingerprint was unchanged and `force` was false.
    Skipped { fingerprint: String },
    /// Full refresh completed and the new batch is now `active`.
    Completed {
        batch_id: i64,
        kept: usize,
        rejected: usize,
        llm_calls: usize,
    },
    /// No points were available in Qdrant to cluster.
    EmptyCorpus,
}

/// Background worker that rebuilds the default-suggestions pool.
pub struct SuggestionRefreshWorker {
    pool: PgPool,
    qdrant: Arc<QdrantClient>,
    clusterer: Arc<dyn Clusterer>,
    llm: Arc<dyn LlmProvider>,
    default_repo: DefaultSuggestionsRepository,
    refresh_repo: RefreshBatchesRepository,
    config: DefaultSuggestionsConfig,
    collection_name: String,
}

impl SuggestionRefreshWorker {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: PgPool,
        qdrant: Arc<QdrantClient>,
        clusterer: Arc<dyn Clusterer>,
        llm: Arc<dyn LlmProvider>,
        default_repo: DefaultSuggestionsRepository,
        refresh_repo: RefreshBatchesRepository,
        config: DefaultSuggestionsConfig,
        collection_name: String,
    ) -> Self {
        Self {
            pool,
            qdrant,
            clusterer,
            llm,
            default_repo,
            refresh_repo,
            config,
            collection_name,
        }
    }

    /// Run the full refresh pipeline once. Pass `force = true` to
    /// rebuild even when the corpus fingerprint hasn't changed.
    pub async fn run_once(&self, force: bool) -> Result<RunSummary> {
        let refresh_cfg = &self.config.refresh;
        let lock_id = refresh_cfg.advisory_lock_id;

        // Pin one connection for the full run so the session-scoped
        // advisory lock stays valid across SQL calls made on it.
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| Error::Database(format!("failed to acquire pg conn: {e}")))?;

        let locked: bool = sqlx::query("SELECT pg_try_advisory_lock($1) AS locked")
            .bind(lock_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| Error::Database(format!("pg_try_advisory_lock failed: {e}")))?
            .get("locked");

        if !locked {
            warn!("suggestion refresh skipped: advisory lock held by another replica");
            return Ok(RunSummary::LockHeld);
        }

        // From here on, anything that returns MUST release the lock.
        let result = self.run_locked(force, refresh_cfg).await;

        if let Err(e) = sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(lock_id)
            .execute(&mut *conn)
            .await
        {
            error!(error = %e, "failed to release advisory lock; continuing");
        }

        result
    }

    async fn run_locked(&self, force: bool, refresh_cfg: &RefreshConfig) -> Result<RunSummary> {
        // --- 1. Sample corpus + fingerprint --------------------------------
        let sample = self
            .qdrant
            .scroll_with_vectors(&self.collection_name, refresh_cfg.sample_cap as u32)
            .await?;

        if sample.is_empty() {
            info!("suggestion refresh: qdrant scroll returned 0 points");
            return Ok(RunSummary::EmptyCorpus);
        }

        let points_count = self
            .qdrant
            .collection_info(&self.collection_name)
            .await
            .unwrap_or(sample.len() as u64);

        let fingerprint = compute_fingerprint(
            &self.collection_name,
            points_count,
            &collect_first_ids(&sample, 32),
        );

        if !force
            && let Some(prev) = self.refresh_repo.latest_active().await?
            && prev.corpus_fingerprint == fingerprint
        {
            info!(fingerprint = %fingerprint, "suggestion refresh skipped: corpus unchanged");
            return Ok(RunSummary::Skipped { fingerprint });
        }

        // --- 2. Start batch -----------------------------------------------
        let batch_id = self.refresh_repo.start_batch(&fingerprint).await?;

        match self.build_and_swap(batch_id, &sample, refresh_cfg).await {
            Ok(summary) => Ok(summary),
            Err(e) => {
                if let Err(mark_err) = self.refresh_repo.mark_failed(batch_id).await {
                    error!(error = %mark_err, batch_id, "failed to mark batch failed");
                }
                Err(e)
            }
        }
    }

    async fn build_and_swap(
        &self,
        batch_id: i64,
        sample: &[ScrolledPoint],
        refresh_cfg: &RefreshConfig,
    ) -> Result<RunSummary> {
        let vectors: Vec<Vec<f32>> = sample.iter().map(|p| p.vector.clone()).collect();
        let clusters = self.clusterer.kmeans(&vectors, refresh_cfg.cluster_count)?;

        let safety_re = Regex::new(&self.config.safety_regex)
            .map_err(|e| Error::Internal(format!("invalid suggestions.safety_regex: {e}")))?;

        let GeneratedRows {
            rows,
            llm_calls,
            kept,
            rejected,
        } = generate_rows_for_clusters(
            self.llm.as_ref(),
            &clusters,
            sample,
            &safety_re,
            batch_id,
            self.config.default_weight,
            Duration::from_millis(refresh_cfg.generation_timeout_ms),
        )
        .await?;

        let inserted = self.default_repo.insert_bulk(&rows).await?;
        info!(
            batch_id,
            kept, rejected, inserted, "suggestion refresh insert_bulk done"
        );

        self.refresh_repo
            .swap_active_atomic(batch_id, llm_calls as i32, kept as i32, rejected as i32)
            .await?;

        let _ = self
            .refresh_repo
            .retain_last_n(refresh_cfg.retention_batches)
            .await?;

        Ok(RunSummary::Completed {
            batch_id,
            kept,
            rejected,
            llm_calls,
        })
    }

    /// Scheduled loop: sleeps until the next time the configured cron
    /// expression fires, then calls `run_once(false)`. Errors are logged,
    /// never propagated. If the cron expression fails to parse the loop
    /// falls back to a fixed daily 03:00 UTC schedule and logs an error.
    pub async fn run_scheduled(self) {
        let cron_expr = self.config.refresh.schedule_cron.clone();
        info!(
            cron = %cron_expr,
            "starting SuggestionRefreshWorker scheduled loop"
        );
        let schedule = match Schedule::from_str(&cron_expr) {
            Ok(s) => Some(s),
            Err(e) => {
                error!(
                    error = %e,
                    cron = %cron_expr,
                    "invalid suggestions.refresh.schedule_cron; falling back to daily 03:00 UTC"
                );
                None
            }
        };

        loop {
            let sleep_secs = seconds_until_next_fire(schedule.as_ref());
            info!(sleep_secs, "suggestion refresh sleeping until next fire");
            time::sleep(Duration::from_secs(sleep_secs)).await;

            match self.run_once(false).await {
                Ok(summary) => info!(?summary, "suggestion refresh cycle done"),
                Err(e) => error!(error = %e, "suggestion refresh cycle failed"),
            }
        }
    }
}

/// Compute seconds-until-next-fire for the given schedule. Falls back to
/// daily 03:00 UTC if the schedule is `None` or has no upcoming time.
fn seconds_until_next_fire(schedule: Option<&Schedule>) -> u64 {
    let now = Utc::now();
    if let Some(s) = schedule
        && let Some(next) = s.upcoming(Utc).next()
    {
        let diff = next.signed_duration_since(now).num_seconds();
        return diff.max(1) as u64;
    }
    seconds_until_next_0300_utc()
}

// -------- helpers --------------------------------------------------------

/// Result of `generate_rows_for_clusters`: the surviving rows plus tallies.
#[derive(Debug)]
struct GeneratedRows {
    rows: Vec<NewDefaultSuggestion>,
    llm_calls: usize,
    kept: usize,
    rejected: usize,
}

/// Run the per-cluster LLM + safety-filter loop. Per-cluster LLM errors
/// are logged and skipped (degraded-but-non-empty refresh is acceptable),
/// but if **every** attempted cluster errors and no rows are produced,
/// this returns `Err` so the caller can mark the batch `failed` and
/// leave the previously-active batch untouched (spec §4.4 step 5,
/// §10.3, QA HIGH fix).
async fn generate_rows_for_clusters(
    llm: &dyn LlmProvider,
    clusters: &[Cluster],
    sample: &[ScrolledPoint],
    safety_re: &Regex,
    batch_id: i64,
    default_weight: i32,
    generation_timeout: Duration,
) -> Result<GeneratedRows> {
    let mut rows: Vec<NewDefaultSuggestion> = Vec::new();
    let mut llm_calls: usize = 0;
    let mut cluster_errors: usize = 0;
    let mut kept: usize = 0;
    let mut rejected: usize = 0;

    for cluster in clusters {
        let excerpt = build_cluster_excerpt(cluster, sample, 5, 2000);
        if excerpt.is_empty() {
            continue;
        }

        llm_calls += 1;
        let cluster_questions = match time::timeout(
            generation_timeout,
            llm.generate_cluster_questions(&excerpt),
        )
        .await
        {
            Ok(Ok(q)) => q,
            Ok(Err(e)) => {
                cluster_errors += 1;
                warn!(error = %e, cluster_id = cluster.id, "generate_cluster_questions failed; skipping cluster");
                continue;
            }
            Err(_) => {
                cluster_errors += 1;
                warn!(
                    cluster_id = cluster.id,
                    timeout_ms = generation_timeout.as_millis() as u64,
                    "generate_cluster_questions timed out; skipping cluster"
                );
                continue;
            }
        };

        for (locale, questions) in &cluster_questions.questions {
            let (good, bad) = safety_filter(questions.clone(), safety_re);
            kept += good.len();
            rejected += bad;
            for q in good {
                rows.push(NewDefaultSuggestion {
                    batch_id,
                    locale: *locale,
                    question: q,
                    topic_cluster_id: cluster.id as i32,
                    topic_label: cluster_questions.label.clone(),
                    weight: default_weight,
                });
            }
        }
    }

    if rows.is_empty() {
        return Err(Error::Internal(format!(
            "refresh produced zero questions ({cluster_errors} cluster errors); aborting batch to preserve previous active state"
        )));
    }

    Ok(GeneratedRows {
        rows,
        llm_calls,
        kept,
        rejected,
    })
}

/// Build a compact excerpt for the LLM prompt: concatenate the top-N
/// members closest to the centroid (in insertion order of the cluster),
/// truncated to `max_chars`.
fn build_cluster_excerpt(
    cluster: &Cluster,
    sample: &[ScrolledPoint],
    top_n: usize,
    max_chars: usize,
) -> String {
    // Sort member indices by cosine-like distance to centroid.
    let mut scored: Vec<(usize, f32)> = cluster
        .member_indices
        .iter()
        .filter_map(|&idx| {
            let p = sample.get(idx)?;
            let d = squared_distance(&p.vector, &cluster.centroid);
            Some((idx, d))
        })
        .collect();
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut out = String::new();
    for (idx, _) in scored.into_iter().take(top_n) {
        if let Some(p) = sample.get(idx) {
            if !out.is_empty() {
                out.push_str("\n\n---\n\n");
            }
            out.push_str(p.text.trim());
            if out.len() >= max_chars {
                out.truncate(max_chars);
                break;
            }
        }
    }
    out
}

fn squared_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return f32::MAX;
    }
    a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum()
}

fn collect_first_ids(sample: &[ScrolledPoint], n: usize) -> Vec<String> {
    let mut ids: Vec<String> = sample.iter().take(n * 4).map(|p| p.id.clone()).collect();
    ids.sort();
    ids.truncate(n);
    ids
}

/// sha256 over `(collection_name, points_count, sorted_first_ids)`.
fn compute_fingerprint(collection_name: &str, points_count: u64, point_ids: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(collection_name.as_bytes());
    hasher.update([0u8]);
    hasher.update(points_count.to_be_bytes());
    for id in point_ids {
        hasher.update([0u8]);
        hasher.update(id.as_bytes());
    }
    let digest = hasher.finalize();
    hex_encode(&digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// Drop questions that match the safety regex, violate length bounds, or
/// collide case-insensitively with an earlier kept entry. Returns
/// `(kept, rejected_count)`.
fn safety_filter(questions: Vec<String>, safety_re: &Regex) -> (Vec<String>, usize) {
    let mut kept: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut rejected = 0usize;

    for q in questions {
        let trimmed = q.trim().to_string();
        let len = trimmed.chars().count();
        if !(5..=120).contains(&len) {
            rejected += 1;
            continue;
        }
        if safety_re.is_match(&trimmed) {
            rejected += 1;
            continue;
        }
        let key = trimmed.to_lowercase();
        if !seen.insert(key) {
            rejected += 1;
            continue;
        }
        kept.push(trimmed);
    }

    (kept, rejected)
}

/// Seconds until the next 03:00 UTC from now. Always returns a positive
/// value (if we're exactly at 03:00 UTC we schedule 24h out).
fn seconds_until_next_0300_utc() -> u64 {
    let now = Utc::now();
    let today_0300 = Utc
        .with_ymd_and_hms(now.year(), now.month(), now.day(), 3, 0, 0)
        .single()
        .unwrap_or(now);
    let target = if now < today_0300 {
        today_0300
    } else {
        today_0300 + chrono::Duration::days(1)
    };
    let diff = target.signed_duration_since(now).num_seconds();
    diff.max(1) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safety_regex() -> Regex {
        Regex::new(
            r"(?i)(\bprice\b|should[\s\-]?i[\s\-]?(buy|sell|invest)|will[\s\w]+hit|\bprediction\b|\bforecast\b|\bpredicted\b|\bpurchase\b|\binvesting\b|\bbuying\b|where[\s\w]+buy)",
        )
        .unwrap()
    }

    #[test]
    fn safety_filter_catches_bypass_vectors() {
        let re = safety_regex();
        let input = vec![
            "How does buying Bitcoin actually work?".to_string(),
            "Investing in DeFi safely".to_string(),
            "What is the predicted outcome for ETH?".to_string(),
            "Where can I buy Bitcoin in Europe?".to_string(),
            "How does proof of stake work?".to_string(),
        ];
        let (kept, rejected) = safety_filter(input, &re);
        assert_eq!(kept, vec!["How does proof of stake work?".to_string()]);
        assert_eq!(rejected, 4);
    }

    #[test]
    fn safety_filter_drops_financial_advice() {
        let re = safety_regex();
        let input = vec![
            "What is the Bitcoin price prediction for 2026?".to_string(),
            "How does proof of stake work?".to_string(),
            "Should I buy ETH now?".to_string(),
        ];
        let (kept, rejected) = safety_filter(input, &re);
        assert_eq!(kept, vec!["How does proof of stake work?".to_string()]);
        assert_eq!(rejected, 2);
    }

    #[test]
    fn safety_filter_length_guard() {
        let re = safety_regex();
        let too_short = "hey".to_string();
        let too_long = "x".repeat(200);
        let ok = "How does staking work on Ethereum?".to_string();
        let (kept, rejected) = safety_filter(vec![too_short, too_long, ok.clone()], &re);
        assert_eq!(kept, vec![ok]);
        assert_eq!(rejected, 2);
    }

    #[test]
    fn safety_filter_case_insensitive_dedup() {
        let re = safety_regex();
        let input = vec![
            "What is a blockchain?".to_string(),
            "what is a BLOCKCHAIN?".to_string(),
            "What is a blockchain?".to_string(),
        ];
        let (kept, rejected) = safety_filter(input, &re);
        assert_eq!(kept.len(), 1);
        assert_eq!(rejected, 2);
    }

    #[test]
    fn compute_fingerprint_deterministic() {
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let fp1 = compute_fingerprint("docs", 1000, &ids);
        let fp2 = compute_fingerprint("docs", 1000, &ids);
        assert_eq!(fp1, fp2);
        assert_eq!(fp1.len(), 64); // sha256 hex
    }

    #[test]
    fn compute_fingerprint_differs_on_count() {
        let ids = vec!["a".to_string(), "b".to_string()];
        let fp1 = compute_fingerprint("docs", 1000, &ids);
        let fp2 = compute_fingerprint("docs", 1001, &ids);
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn compute_fingerprint_differs_on_ids() {
        let fp1 = compute_fingerprint("docs", 1000, &["a".to_string()]);
        let fp2 = compute_fingerprint("docs", 1000, &["b".to_string()]);
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn safety_filter_does_not_reject_investigate() {
        // Word-boundary tightening: "investigate" must NOT be caught by the
        // `\binvesting\b` / `should i invest` patterns.
        let re = safety_regex();
        let input = vec![
            "How to investigate a memory leak?".to_string(),
            "How do I investigate slow SQL queries?".to_string(),
        ];
        let (kept, rejected) = safety_filter(input, &re);
        assert_eq!(kept.len(), 2);
        assert_eq!(rejected, 0);
    }

    #[test]
    fn safety_filter_still_catches_investing_and_price() {
        // Prove the word-boundary regex still traps the real targets.
        let re = safety_regex();
        let input = vec![
            "Investing in DeFi in 2026".to_string(),
            "What is the current BTC price today?".to_string(),
            "How does proof of stake work?".to_string(),
        ];
        let (kept, rejected) = safety_filter(input, &re);
        assert_eq!(kept, vec!["How does proof of stake work?".to_string()]);
        assert_eq!(rejected, 2);
    }

    // -------- mock LlmProvider for generate_rows_for_clusters tests --------

    use async_trait::async_trait;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures::Stream;
    use kenjaku_core::error::Error as CoreError;
    use kenjaku_core::traits::llm::LlmProvider;
    use kenjaku_core::types::locale::Locale;
    use kenjaku_core::types::search::{LlmResponse, StreamChunk, TranslationResult};
    use kenjaku_core::types::suggestion::ClusterQuestions;

    /// Mock that errors on the first `fail_first` calls then succeeds.
    struct MockLlm {
        calls: AtomicUsize,
        fail_first: usize,
    }

    #[async_trait]
    impl LlmProvider for MockLlm {
        async fn generate(
            &self,
            _messages: &[kenjaku_core::types::message::Message],
        ) -> kenjaku_core::error::Result<LlmResponse> {
            unimplemented!()
        }
        async fn generate_stream(
            &self,
            _messages: &[kenjaku_core::types::message::Message],
        ) -> kenjaku_core::error::Result<
            Pin<Box<dyn Stream<Item = kenjaku_core::error::Result<StreamChunk>> + Send>>,
        > {
            unimplemented!()
        }
        async fn translate(&self, _t: &str) -> kenjaku_core::error::Result<TranslationResult> {
            unimplemented!()
        }
        async fn suggest(&self, _q: &str, _a: &str) -> kenjaku_core::error::Result<Vec<String>> {
            unimplemented!()
        }
        async fn generate_cluster_questions(
            &self,
            _excerpt: &str,
        ) -> kenjaku_core::error::Result<ClusterQuestions> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_first {
                return Err(CoreError::Internal("mock llm boom".to_string()));
            }
            let mut questions = std::collections::HashMap::new();
            questions.insert(
                Locale::En,
                vec![
                    "How does proof of stake work?".to_string(),
                    "What is a blockchain finality guarantee?".to_string(),
                ],
            );
            Ok(ClusterQuestions {
                label: "staking".to_string(),
                questions,
            })
        }
    }

    fn dummy_sample(n: usize) -> Vec<ScrolledPoint> {
        (0..n)
            .map(|i| ScrolledPoint {
                id: format!("p{i}"),
                vector: vec![0.0; 4],
                text: format!("document text {i} about staking rewards"),
            })
            .collect()
    }

    fn dummy_clusters(n: usize, sample_len: usize) -> Vec<Cluster> {
        (0..n)
            .map(|i| Cluster {
                id: i,
                member_indices: (0..sample_len).collect(),
                centroid: vec![0.0; 4],
            })
            .collect()
    }

    #[tokio::test]
    async fn generate_rows_aborts_when_all_clusters_fail() {
        let llm = MockLlm {
            calls: AtomicUsize::new(0),
            fail_first: usize::MAX, // always fail
        };
        let sample = dummy_sample(3);
        let clusters = dummy_clusters(3, sample.len());
        let re = safety_regex();

        let err = generate_rows_for_clusters(
            &llm,
            &clusters,
            &sample,
            &re,
            42,
            10,
            Duration::from_secs(5),
        )
        .await
        .expect_err("expected all-failed abort");
        let msg = err.to_string();
        assert!(
            msg.contains("zero questions") && msg.contains("3 cluster errors"),
            "unexpected error: {msg}"
        );
    }

    /// Mock that always succeeds but returns only questions that the
    /// safety regex will reject. Exercises the "no errors but empty
    /// rows" path that was previously letting empty batches through.
    struct DenyOnlyLlm;

    #[async_trait]
    impl LlmProvider for DenyOnlyLlm {
        async fn generate(
            &self,
            _messages: &[kenjaku_core::types::message::Message],
        ) -> kenjaku_core::error::Result<LlmResponse> {
            unimplemented!()
        }
        async fn generate_stream(
            &self,
            _messages: &[kenjaku_core::types::message::Message],
        ) -> kenjaku_core::error::Result<
            Pin<Box<dyn Stream<Item = kenjaku_core::error::Result<StreamChunk>> + Send>>,
        > {
            unimplemented!()
        }
        async fn translate(&self, _t: &str) -> kenjaku_core::error::Result<TranslationResult> {
            unimplemented!()
        }
        async fn suggest(&self, _q: &str, _a: &str) -> kenjaku_core::error::Result<Vec<String>> {
            unimplemented!()
        }
        async fn generate_cluster_questions(
            &self,
            _excerpt: &str,
        ) -> kenjaku_core::error::Result<ClusterQuestions> {
            let mut questions = std::collections::HashMap::new();
            questions.insert(
                Locale::En,
                vec![
                    "What is the BTC price prediction for next year?".to_string(),
                    "Should I buy ETH right now?".to_string(),
                ],
            );
            Ok(ClusterQuestions {
                label: "advice".to_string(),
                questions,
            })
        }
    }

    #[tokio::test]
    async fn generate_rows_aborts_when_safety_filter_rejects_everything() {
        let llm = DenyOnlyLlm;
        let sample = dummy_sample(3);
        let clusters = dummy_clusters(2, sample.len());
        let re = safety_regex();

        let err = generate_rows_for_clusters(
            &llm,
            &clusters,
            &sample,
            &re,
            99,
            10,
            Duration::from_secs(5),
        )
        .await
        .expect_err("expected empty-rows abort");
        let msg = err.to_string();
        assert!(
            msg.contains("zero questions") && msg.contains("0 cluster errors"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn generate_rows_completes_with_partial_failures() {
        // 4 clusters, first 2 LLM calls fail, last 2 succeed.
        let llm = MockLlm {
            calls: AtomicUsize::new(0),
            fail_first: 2,
        };
        let sample = dummy_sample(3);
        let clusters = dummy_clusters(4, sample.len());
        let re = safety_regex();

        let out = generate_rows_for_clusters(
            &llm,
            &clusters,
            &sample,
            &re,
            7,
            10,
            Duration::from_secs(5),
        )
        .await
        .expect("partial failure should still return Ok");
        assert_eq!(out.llm_calls, 4);
        assert!(out.kept > 0, "expected kept rows from successful clusters");
        assert!(
            !out.rows.is_empty(),
            "expected non-empty rows for partial success"
        );
        assert!(out.rows.iter().all(|r| r.batch_id == 7));
    }

    /// Mock that sleeps far longer than any plausible timeout, used to
    /// exercise the per-cluster `tokio::time::timeout` guard.
    struct SleepingLlm;

    #[async_trait]
    impl LlmProvider for SleepingLlm {
        async fn generate(
            &self,
            _messages: &[kenjaku_core::types::message::Message],
        ) -> kenjaku_core::error::Result<LlmResponse> {
            unimplemented!()
        }
        async fn generate_stream(
            &self,
            _messages: &[kenjaku_core::types::message::Message],
        ) -> kenjaku_core::error::Result<
            Pin<Box<dyn Stream<Item = kenjaku_core::error::Result<StreamChunk>> + Send>>,
        > {
            unimplemented!()
        }
        async fn translate(&self, _t: &str) -> kenjaku_core::error::Result<TranslationResult> {
            unimplemented!()
        }
        async fn suggest(&self, _q: &str, _a: &str) -> kenjaku_core::error::Result<Vec<String>> {
            unimplemented!()
        }
        async fn generate_cluster_questions(
            &self,
            _excerpt: &str,
        ) -> kenjaku_core::error::Result<ClusterQuestions> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(ClusterQuestions::default())
        }
    }

    #[tokio::test]
    async fn generate_rows_aborts_when_all_clusters_timeout() {
        let llm = SleepingLlm;
        let sample = dummy_sample(3);
        let clusters = dummy_clusters(2, sample.len());
        let re = safety_regex();

        let err = generate_rows_for_clusters(
            &llm,
            &clusters,
            &sample,
            &re,
            123,
            10,
            Duration::from_millis(20),
        )
        .await
        .expect_err("expected timeout-induced abort");
        let msg = err.to_string();
        assert!(
            msg.contains("zero questions") && msg.contains("2 cluster errors"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn seconds_until_next_0300_utc_is_positive_and_bounded() {
        let s = seconds_until_next_0300_utc();
        assert!(s > 0);
        assert!(s <= 24 * 3600 + 1);
    }

    #[test]
    fn seconds_until_next_fire_uses_cron_when_present() {
        // 6-field cron: every second. Next fire should be <=1s away.
        let schedule = Schedule::from_str("* * * * * *").expect("valid cron");
        let s = seconds_until_next_fire(Some(&schedule));
        assert!(s <= 2, "expected near-immediate fire, got {s}");
    }

    #[test]
    fn seconds_until_next_fire_falls_back_to_0300_when_none() {
        let s = seconds_until_next_fire(None);
        assert!(s > 0);
        assert!(s <= 24 * 3600 + 1);
    }
}
