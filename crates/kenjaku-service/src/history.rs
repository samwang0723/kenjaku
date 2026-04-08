//! In-memory per-session conversation history store.
//!
//! Purpose: give the LLM follow-up context on repeat calls from the same
//! session without hitting Postgres. This is a runtime cache, NOT a
//! durable record — the authoritative `conversations` table is still
//! written by `ConversationService::record` via the async flush pipeline.
//!
//! Design (see user decisions recorded in the session):
//! - Per-session FIFO deque, capped at `max_turns_per_session`
//! - DashMap so per-session writes don't contend on a global lock
//! - Background janitor evicts sessions idle longer than
//!   `session_idle_ttl_seconds` — prevents unbounded memory growth from
//!   abandoned clients
//! - No cross-instance sharing (single replica only, by design)
//! - No persistence across restarts (accepted — PG still has the durable log)

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::time::interval;
use tracing::{debug, info};

use kenjaku_core::config::HistoryConfig;
use kenjaku_core::types::conversation::ConversationTurn;

#[derive(Debug)]
struct SessionEntry {
    turns: std::collections::VecDeque<ConversationTurn>,
    last_touched: Instant,
}

/// Thread-safe in-memory conversation history store.
#[derive(Clone)]
pub struct SessionHistoryStore {
    inner: Arc<DashMap<String, SessionEntry>>,
    config: HistoryConfig,
}

impl SessionHistoryStore {
    pub fn new(config: HistoryConfig) -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            config,
        }
    }

    /// Append a new turn to the session's history. If the session is
    /// over `max_turns_per_session` after the push, the oldest turn is
    /// dropped. No-op when `config.enabled` is false or the session id
    /// is empty.
    pub fn append(&self, session_id: &str, turn: ConversationTurn) {
        if !self.config.enabled || session_id.is_empty() {
            return;
        }
        let cap = self.config.max_turns_per_session.max(1);
        let mut entry = self
            .inner
            .entry(session_id.to_string())
            .or_insert_with(|| SessionEntry {
                turns: std::collections::VecDeque::with_capacity(cap),
                last_touched: Instant::now(),
            });
        entry.turns.push_back(turn);
        while entry.turns.len() > cap {
            entry.turns.pop_front();
        }
        entry.last_touched = Instant::now();
    }

    /// Snapshot up to `inject_max_turns` most-recent turns for the given
    /// session in chronological order (oldest first). Returns an empty
    /// Vec when disabled, session is empty, or nothing has been recorded.
    ///
    /// Touches `last_touched` so active sessions aren't evicted mid-flow.
    pub fn snapshot_for_llm(&self, session_id: &str) -> Vec<ConversationTurn> {
        if !self.config.enabled || session_id.is_empty() {
            return Vec::new();
        }
        let limit = self.config.inject_max_turns;
        if limit == 0 {
            return Vec::new();
        }
        let mut entry = match self.inner.get_mut(session_id) {
            Some(e) => e,
            None => return Vec::new(),
        };
        entry.last_touched = Instant::now();
        let start = entry.turns.len().saturating_sub(limit);
        entry.turns.iter().skip(start).cloned().collect()
    }

    /// Spawn a background janitor that evicts idle sessions. Returns
    /// immediately; the task runs for the process lifetime.
    pub fn spawn_janitor(self) {
        if !self.config.enabled {
            return;
        }
        let ttl = Duration::from_secs(self.config.session_idle_ttl_seconds);
        // Scan at 1/10 of TTL, capped to [60s, 600s].
        let scan_every = ttl
            .checked_div(10)
            .unwrap_or(Duration::from_secs(60))
            .max(Duration::from_secs(60))
            .min(Duration::from_secs(600));
        info!(
            ttl_secs = ttl.as_secs(),
            scan_every_secs = scan_every.as_secs(),
            "SessionHistoryStore janitor starting"
        );
        tokio::spawn(async move {
            let mut ticker = interval(scan_every);
            loop {
                ticker.tick().await;
                let before = self.inner.len();
                self.inner
                    .retain(|_, entry| entry.last_touched.elapsed() < ttl);
                let after = self.inner.len();
                if before != after {
                    debug!(
                        evicted = before - after,
                        remaining = after,
                        "SessionHistoryStore janitor swept idle sessions"
                    );
                }
            }
        });
    }

    #[cfg(test)]
    pub fn session_count(&self) -> usize {
        self.inner.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(user: &str, assistant: &str) -> ConversationTurn {
        ConversationTurn {
            user: user.to_string(),
            assistant: assistant.to_string(),
        }
    }

    fn cfg(max: usize, inject: usize) -> HistoryConfig {
        HistoryConfig {
            enabled: true,
            max_turns_per_session: max,
            inject_max_turns: inject,
            session_idle_ttl_seconds: 3600,
        }
    }

    #[test]
    fn append_caps_at_max_and_drops_oldest() {
        let store = SessionHistoryStore::new(cfg(3, 3));
        for i in 0..5 {
            store.append("s1", turn(&format!("q{i}"), &format!("a{i}")));
        }
        let snap = store.snapshot_for_llm("s1");
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].user, "q2");
        assert_eq!(snap[2].user, "q4");
    }

    #[test]
    fn snapshot_respects_inject_cap() {
        let store = SessionHistoryStore::new(cfg(10, 2));
        for i in 0..5 {
            store.append("s1", turn(&format!("q{i}"), &format!("a{i}")));
        }
        let snap = store.snapshot_for_llm("s1");
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].user, "q3");
        assert_eq!(snap[1].user, "q4");
    }

    #[test]
    fn disabled_is_noop() {
        let mut c = cfg(5, 5);
        c.enabled = false;
        let store = SessionHistoryStore::new(c);
        store.append("s1", turn("q", "a"));
        assert!(store.snapshot_for_llm("s1").is_empty());
        assert_eq!(store.session_count(), 0);
    }

    #[test]
    fn empty_session_id_is_noop() {
        let store = SessionHistoryStore::new(cfg(5, 5));
        store.append("", turn("q", "a"));
        assert_eq!(store.session_count(), 0);
    }

    #[test]
    fn sessions_are_isolated() {
        let store = SessionHistoryStore::new(cfg(5, 5));
        store.append("s1", turn("q1", "a1"));
        store.append("s2", turn("q2", "a2"));
        assert_eq!(store.snapshot_for_llm("s1").len(), 1);
        assert_eq!(store.snapshot_for_llm("s1")[0].user, "q1");
        assert_eq!(store.snapshot_for_llm("s2")[0].user, "q2");
    }
}
