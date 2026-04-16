//! Per-request LLM usage + cost accounting.
//!
//! A search request typically triggers several independent LLM calls
//! (translator, intent classifier, answer generator, suggest). Each
//! underlying call returns its own token counts and model identifier;
//! [`LlmCall`] wraps a single call's measurements, and [`UsageStats`]
//! aggregates all calls made on behalf of one HTTP request.
//!
//! Populated inside the search pipeline and surfaced on
//! `SearchResponse.metadata.usage` (non-streaming) and
//! `StreamDoneMetadata.usage` (streaming). Exposing this from the API
//! lets operators track per-request cost and spot expensive queries
//! without parsing logs.
//!
//! `UsageStats` is append-only: callers either build one imperatively
//! via [`UsageStats::push`] or — in concurrent contexts — share a
//! [`SharedUsageTracker`] and drain it at the end of the request.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Round a USD value to 6 decimal places to avoid JSON float artifacts
/// like `0.000859999999`. Cost estimates only need microdollar
/// resolution; anything beyond that is meaningless f64 drift from
/// repeated floating-point addition.
#[inline]
fn round_6dp(x: f64) -> f64 {
    (x * 1_000_000.0).round() / 1_000_000.0
}

/// Per-call LLM accounting entry.
///
/// One instance per LLM invocation. `purpose` identifies which pipeline
/// step the call served — e.g. `"translate"`, `"classify_intent"`,
/// `"generate"`, `"suggest"`. `cost_usd` is the tier-adjusted estimate
/// produced by the provider (see `GeminiProvider::estimate_cost`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmCall {
    /// Pipeline step that issued the call. Free-form but should be
    /// stable across releases so dashboards can group by it.
    pub purpose: String,
    /// Provider-reported model identifier (e.g. `"gemini-2.5-flash"`).
    pub model: String,
    /// Prompt tokens consumed by this call.
    pub input_tokens: u32,
    /// Completion tokens produced by this call.
    pub output_tokens: u32,
    /// Tier-adjusted cost estimate in USD. `0.0` when pricing data is
    /// unavailable for the model — never `NaN` or negative.
    pub cost_usd: f64,
    /// Wall-clock latency of the call in milliseconds (from just
    /// before the provider HTTP request until the response parse
    /// completes).
    pub latency_ms: u64,
}

/// Aggregated LLM accounting for a single search request.
///
/// Fields are the running totals across every [`LlmCall`] in `calls`.
/// The invariant `total_tokens == input_tokens + output_tokens` is
/// maintained by [`UsageStats::push`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageStats {
    /// Sum of `input_tokens` across all calls.
    pub input_tokens: u32,
    /// Sum of `output_tokens` across all calls.
    pub output_tokens: u32,
    /// `input_tokens + output_tokens`.
    pub total_tokens: u32,
    /// Sum of `cost_usd` across all calls.
    pub estimated_cost_usd: f64,
    /// Individual calls in the order they were recorded.
    pub calls: Vec<LlmCall>,
}

impl UsageStats {
    /// Create an empty stats accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a single [`LlmCall`] and update running totals.
    ///
    /// `estimated_cost_usd` is rounded to 6 decimals after each add so
    /// repeated f64 accumulation doesn't surface artifacts like
    /// `0.000859999999` in the JSON response. Per-call `cost_usd`
    /// should already be rounded at construction time (see
    /// `GeminiProvider::estimate_cost`); this is a defense-in-depth
    /// guard.
    pub fn push(&mut self, call: LlmCall) {
        self.input_tokens = self.input_tokens.saturating_add(call.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(call.output_tokens);
        self.total_tokens = self.input_tokens.saturating_add(self.output_tokens);
        self.estimated_cost_usd = round_6dp(self.estimated_cost_usd + call.cost_usd);
        self.calls.push(call);
    }
}

/// Thread-safe, shareable wrapper around a [`UsageStats`] accumulator.
///
/// Cheap to clone (`Arc` bump). Used by the pipeline so that the
/// translator, classifier, generator, and suggest calls — any of which
/// may run concurrently via `tokio::join!` — can push `LlmCall`
/// records without additional plumbing.
#[derive(Debug, Clone, Default)]
pub struct SharedUsageTracker {
    inner: Arc<Mutex<UsageStats>>,
}

impl SharedUsageTracker {
    /// Construct a fresh tracker holding an empty [`UsageStats`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a single call. Non-blocking apart from the mutex acquire
    /// (held only for the append).
    ///
    /// Uses `parking_lot::Mutex` rather than `std::sync::Mutex` for two
    /// reasons. First, concurrent `tokio::join!` brain calls
    /// (translator, classifier, generator) share this tracker; a
    /// poisoned std mutex would silently drop telemetry. Second, the
    /// critical section is microseconds — one `Vec::push` plus a few
    /// integer adds — so a blocking lock beats `tokio::sync::Mutex`
    /// for this workload: no await points, no async overhead.
    /// `parking_lot` is also non-poisoning.
    pub fn record(&self, call: LlmCall) {
        self.inner.lock().push(call);
    }

    /// Consume the tracker and return the accumulated stats. Intended
    /// to be called once at the end of request processing. Falls back
    /// to a clone if the `Arc` still has outstanding references (e.g.
    /// the tracker was leaked into a detached task).
    pub fn into_stats(self) -> UsageStats {
        Arc::try_unwrap(self.inner)
            .map(|m| m.into_inner())
            .unwrap_or_else(|arc| arc.lock().clone())
    }

    /// Snapshot the accumulator without consuming it.
    pub fn snapshot(&self) -> UsageStats {
        self.inner.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_call(purpose: &str, input: u32, output: u32, cost: f64) -> LlmCall {
        LlmCall {
            purpose: purpose.to_string(),
            model: "gemini-test".to_string(),
            input_tokens: input,
            output_tokens: output,
            cost_usd: cost,
            latency_ms: 100,
        }
    }

    #[test]
    fn usage_stats_default_is_empty() {
        let stats = UsageStats::new();
        assert_eq!(stats.input_tokens, 0);
        assert_eq!(stats.output_tokens, 0);
        assert_eq!(stats.total_tokens, 0);
        assert_eq!(stats.estimated_cost_usd, 0.0);
        assert!(stats.calls.is_empty());
    }

    #[test]
    fn push_accumulates_totals() {
        let mut stats = UsageStats::new();
        stats.push(sample_call("translate", 100, 20, 0.001));
        stats.push(sample_call("generate", 500, 300, 0.01));

        assert_eq!(stats.input_tokens, 600);
        assert_eq!(stats.output_tokens, 320);
        assert_eq!(stats.total_tokens, 920);
        assert!((stats.estimated_cost_usd - 0.011).abs() < 1e-9);
        assert_eq!(stats.calls.len(), 2);
        assert_eq!(stats.calls[0].purpose, "translate");
        assert_eq!(stats.calls[1].purpose, "generate");
    }

    #[test]
    fn shared_tracker_records_and_drains() {
        let tracker = SharedUsageTracker::new();
        tracker.record(sample_call("classify_intent", 50, 5, 0.0001));
        tracker.record(sample_call("generate", 1000, 500, 0.02));

        let stats = tracker.into_stats();
        assert_eq!(stats.calls.len(), 2);
        assert_eq!(stats.total_tokens, 1555);
    }

    #[test]
    fn shared_tracker_snapshot_does_not_consume() {
        let tracker = SharedUsageTracker::new();
        tracker.record(sample_call("translate", 10, 10, 0.0));
        let snap = tracker.snapshot();
        assert_eq!(snap.calls.len(), 1);
        // still usable afterwards
        tracker.record(sample_call("generate", 20, 20, 0.0));
        let snap2 = tracker.snapshot();
        assert_eq!(snap2.calls.len(), 2);
    }

    #[test]
    fn shared_tracker_clone_shares_backing_store() {
        let tracker = SharedUsageTracker::new();
        let clone = tracker.clone();
        tracker.record(sample_call("a", 1, 1, 0.0));
        clone.record(sample_call("b", 2, 2, 0.0));
        let stats = tracker.into_stats();
        assert_eq!(stats.calls.len(), 2);
    }

    /// Repeated f64 accumulation produces JSON artifacts like
    /// `0.000859999999`. `push` now rounds to 6 decimals on each add,
    /// so the surfaced `estimated_cost_usd` stays at microdollar
    /// resolution without drift.
    #[test]
    fn push_rounds_cost_to_six_decimals() {
        let mut stats = UsageStats::new();
        // Three adds that, without rounding, produce
        // 0.0001 + 0.0003 + 0.00046 = 0.0008599999999999999 in f64.
        stats.push(sample_call("a", 1, 1, 0.0001));
        stats.push(sample_call("b", 1, 1, 0.0003));
        stats.push(sample_call("c", 1, 1, 0.00046));

        // Exact equality: the rounded result is 0.00086 on the nose.
        assert_eq!(stats.estimated_cost_usd, 0.00086);

        // JSON must not carry the drift either.
        let json = serde_json::to_string(&stats).unwrap();
        assert!(
            json.contains("\"estimated_cost_usd\":0.00086"),
            "serialized cost drifted: {json}"
        );
    }

    #[test]
    fn push_saturates_on_token_overflow() {
        let mut stats = UsageStats::new();
        stats.input_tokens = u32::MAX - 1;
        stats.push(LlmCall {
            purpose: "overflow".into(),
            model: "m".into(),
            input_tokens: 10,
            output_tokens: 0,
            cost_usd: 0.0,
            latency_ms: 0,
        });
        // Saturating: never panics, maxes out at u32::MAX.
        assert_eq!(stats.input_tokens, u32::MAX);
    }

    #[test]
    fn llm_call_serializes_json() {
        let call = sample_call("generate", 100, 50, 0.005);
        let v = serde_json::to_value(&call).unwrap();
        assert_eq!(v.get("purpose").unwrap(), "generate");
        assert_eq!(v.get("input_tokens").unwrap(), 100);
        assert_eq!(v.get("output_tokens").unwrap(), 50);
        assert_eq!(v.get("latency_ms").unwrap(), 100);
    }
}
