use async_trait::async_trait;
use serde_json::Value;

use unly_core::{tool::ToolSchema, Result};

use crate::manifest::PluginManifest;

/// A plugin event.
#[derive(Debug, Clone)]
pub enum PluginEvent {
    /// Platform is starting up.
    Startup,
    /// Platform is shutting down.
    Shutdown,
    /// A message was received (chat_id, user_id, content).
    MessageReceived {
        chat_id: String,
        user_id: Option<String>,
        content: String,
    },
    /// A tool was executed.
    ToolExecuted {
        tool_name: String,
        success: bool,
    },
    /// Custom event payload.
    Custom { event_type: String, payload: Value },
}

/// A command registered by a plugin.
#[derive(Debug, Clone)]
pub struct PluginCommand {
    pub name: String,
    pub description: String,
    pub usage: String,
}

/// A job registered by a plugin.
#[derive(Debug, Clone)]
pub struct PluginJob {
    pub name: String,
    pub cron_expression: String,
    pub description: String,
}

/// The plugin trait that all plugins must implement.
#[async_trait]
pub trait Plugin: Send + Sync {
    /// Return the plugin manifest.
    fn manifest(&self) -> &PluginManifest;

    /// Initialize the plugin with its configuration.
    async fn init(&mut self, config: Value) -> Result<()>;

    /// Clean up resources.
    async fn shutdown(&self) -> Result<()>;

    /// Return tool schemas this plugin provides (if any).
    fn tools(&self) -> Vec<ToolSchema> {
        Vec::new()
    }

    /// Return commands this plugin registers (if any).
    fn commands(&self) -> Vec<PluginCommand> {
        Vec::new()
    }

    /// Return background jobs this plugin registers (if any).
    fn jobs(&self) -> Vec<PluginJob> {
        Vec::new()
    }

    /// Handle a platform event.
    async fn on_event(&self, event: &PluginEvent) -> Result<()> {
        Ok(())
    }

    /// Execute a plugin command. Returns the response text.
    async fn execute_command(&self, command: &str, args: &str, chat_id: &str) -> Result<String> {
        Ok(format!("Command '{}' not implemented", command))
    }
}
