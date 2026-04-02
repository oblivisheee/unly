//! Telegram bot interface for the unly agent platform.
//!
//! Implements:
//! - Slash command handlers (/start, /help, /model, /provider, /status, /subagent, /subagents, /approve, /deny)
//! - Inline keyboard support
//! - Per-chat session isolation
//! - User permission enforcement
//! - Rate limiting
//! - Message streaming (incremental edit updates)

pub mod bot;
pub mod commands;
pub mod error;
pub mod permissions;
pub mod session;

pub use bot::TelegramBot;
pub use error::TelegramError;
pub use session::SessionStore;
