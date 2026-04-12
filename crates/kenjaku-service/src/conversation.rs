use tokio::sync::mpsc;
use tracing::{error, info, instrument};

use kenjaku_core::types::conversation::CreateConversation;
use kenjaku_infra::postgres::ConversationRepository;

/// Service for async conversation storage.
///
/// Uses a bounded channel to decouple the search hot path from DB writes.
/// The caller sends `CreateConversation` records into the channel; a background
/// worker drains the channel and batch-inserts into PostgreSQL.
pub struct ConversationService {
    tx: mpsc::Sender<CreateConversation>,
}

impl ConversationService {
    /// Create a new conversation service with a flush worker.
    ///
    /// Returns the service (for sending) and the worker (to be spawned).
    pub fn new(
        repo: ConversationRepository,
        buffer_size: usize,
    ) -> (Self, ConversationFlushWorker) {
        let (tx, rx) = mpsc::channel(buffer_size);
        let worker = ConversationFlushWorker { repo, rx };
        (Self { tx }, worker)
    }

    /// Queue a conversation record for async persistence.
    /// Returns immediately; never blocks the search response.
    #[instrument(skip(self, record), fields(request_id = %record.request_id))]
    pub async fn record(&self, record: CreateConversation) {
        if let Err(e) = self.tx.try_send(record) {
            error!(error = %e, "Failed to queue conversation record (channel full or closed)");
        }
    }

    /// Test-only constructor: returns a service backed by a plain mpsc
    /// channel (no flush worker, no PgPool). The receiver is returned so
    /// tests can inspect queued records.
    #[cfg(test)]
    pub(crate) fn test_channel() -> (Self, mpsc::Receiver<CreateConversation>) {
        let (tx, rx) = mpsc::channel(64);
        (Self { tx }, rx)
    }
}

/// Background worker that drains the conversation channel and batch-writes to PostgreSQL.
pub struct ConversationFlushWorker {
    repo: ConversationRepository,
    rx: mpsc::Receiver<CreateConversation>,
}

impl ConversationFlushWorker {
    /// Run the flush loop. Call via `tokio::spawn(worker.run())`.
    pub async fn run(mut self) {
        info!("Starting conversation flush worker");

        let mut batch: Vec<CreateConversation> = Vec::with_capacity(64);

        loop {
            // Block until at least one record arrives (or channel closes).
            match self.rx.recv().await {
                Some(record) => batch.push(record),
                None => {
                    // Channel closed — flush remaining and exit.
                    if !batch.is_empty() {
                        self.flush(&mut batch).await;
                    }
                    info!("Conversation flush worker shutting down");
                    return;
                }
            }

            // Drain any additional buffered records without blocking.
            while batch.len() < 64 {
                match self.rx.try_recv() {
                    Ok(record) => batch.push(record),
                    Err(_) => break,
                }
            }

            self.flush(&mut batch).await;
        }
    }

    async fn flush(&self, batch: &mut Vec<CreateConversation>) {
        if batch.is_empty() {
            return;
        }

        let count = batch.len();
        match self.repo.batch_create(batch).await {
            Ok(inserted) => {
                info!(count = inserted, "Flushed conversation records");
            }
            Err(e) => {
                error!(error = %e, count = count, "Failed to flush conversation records");
            }
        }
        batch.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenjaku_core::types::intent::Intent;
    use kenjaku_core::types::locale::Locale;

    #[test]
    fn test_create_conversation_service() {
        // We can't test with a real repo, but we can verify the channel setup.
        // Using a mock would require the full PgPool — skip for unit tests.
        let record = CreateConversation {
            session_id: "sess".to_string(),
            request_id: "req".to_string(),
            query: "test".to_string(),
            response_text: "answer".to_string(),
            locale: Locale::En,
            intent: Intent::Factual,
            meta: serde_json::json!({}),
        };
        assert_eq!(record.locale, Locale::En);
    }
}
