//! Append-only audit logging pipeline for the unly agent platform.
//!
//! Every security-relevant event is recorded in the audit_log table with:
//! - event type
//! - subject (who triggered the event)
//! - action (what was attempted)
//! - outcome (success / failure / denied)
//! - structured details

pub mod audit;
pub mod event;

pub use audit::AuditLogger;
pub use event::{AuditEvent, AuditOutcome};
