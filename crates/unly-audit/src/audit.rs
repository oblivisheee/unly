use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

use unly_db::{repo::audit::AuditRow, Database};

use crate::event::AuditEvent;

/// The audit logger — appends events to the audit_log table.
///
/// Events are queued and written asynchronously to avoid blocking callers.
#[derive(Clone)]
pub struct AuditLogger {
    sender: mpsc::UnboundedSender<AuditEvent>,
}

impl AuditLogger {
    /// Create an AuditLogger and start its background writer task.
    pub fn new(db: Database) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<AuditEvent>();

        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
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
            warn!("audit logger channel closed");
        });

        Self { sender: tx }
    }

    /// Log an audit event (non-blocking).
    pub fn log(&self, event: AuditEvent) {
        if let Err(e) = self.sender.send(event) {
            error!("failed to queue audit event: {}", e);
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
