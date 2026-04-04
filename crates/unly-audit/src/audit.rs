use tokio::sync::{mpsc, oneshot};
use tracing::{error, warn};
use uuid::Uuid;

use unly_db::{Database, repo::audit::AuditRow};

use crate::event::AuditEvent;

enum AuditCommand {
    Event(AuditEvent),
    Flush(oneshot::Sender<()>),
}

/// The audit logger — appends events to the audit_log table.
///
/// Events are queued and written asynchronously to avoid blocking callers.
#[derive(Clone)]
pub struct AuditLogger {
    sender: mpsc::UnboundedSender<AuditCommand>,
}

impl AuditLogger {
    /// Create an AuditLogger and start its background writer task.
    pub fn new(db: Database) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<AuditCommand>();

        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    AuditCommand::Event(event) => {
                        let row = AuditRow {
                            id: Uuid::new_v4().to_string(),
                            event_type: event.event_type.clone(),
                            user_id: event.user_id.clone(),
                            chat_id: event.chat_id.clone(),
                            agent_id: event.agent_id.clone(),
                            subject: event.subject.clone(),
                            action: event.action.clone(),
                            outcome: event.outcome.to_string(),
                            details: serde_json::to_string(&event.details).unwrap_or_default(),
                            created_at: event.timestamp,
                        };
                        let repo = unly_db::repo::audit::AuditRepo::new(db.conn());
                        if let Err(e) = repo.insert(&row).await {
                            error!("failed to write audit log entry: {}", e);
                        }
                    }
                    AuditCommand::Flush(done_tx) => {
                        let _ = done_tx.send(());
                    }
                }
            }
            warn!("audit logger channel closed");
        });

        Self { sender: tx }
    }

    /// Log an audit event (non-blocking).
    pub fn log(&self, event: AuditEvent) {
        if let Err(e) = self.sender.send(AuditCommand::Event(event)) {
            error!("failed to queue audit event: {}", e);
        }
    }

    /// Flush all queued audit events.
    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        if let Err(e) = self.sender.send(AuditCommand::Flush(tx)) {
            error!("failed to queue audit flush: {}", e);
            return;
        }
        if let Err(e) = rx.await {
            error!("failed waiting for audit flush: {}", e);
        }
    }

    /// Log a success event.
    pub fn success(
        &self,
        event_type: impl Into<String>,
        subject: impl Into<String>,
        action: impl Into<String>,
    ) {
        self.log(AuditEvent::success(event_type, subject, action));
    }

    /// Log a denied event.
    pub fn denied(
        &self,
        event_type: impl Into<String>,
        subject: impl Into<String>,
        action: impl Into<String>,
        reason: impl Into<String>,
    ) {
        self.log(AuditEvent::denied(event_type, subject, action, reason));
    }

    /// Log a failure event.
    pub fn failure(
        &self,
        event_type: impl Into<String>,
        subject: impl Into<String>,
        action: impl Into<String>,
        error: impl Into<String>,
    ) {
        self.log(AuditEvent::failure(event_type, subject, action, error));
    }
}
