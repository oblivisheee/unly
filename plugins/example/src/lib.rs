//! Example plugin for the unly agent platform.
//!
//! Demonstrates:
//! - Plugin manifest
//! - Custom slash command
//! - Lifecycle hooks (init / shutdown / event handling)
//! - Background job registration

use async_trait::async_trait;
use tracing::{info, warn};

use unly_core::{tool::ToolSchema, Result};
use unly_plugins::{
    manifest::PluginManifest,
    plugin::{Plugin, PluginCommand, PluginEvent, PluginJob},
};

/// The example plugin.
pub struct ExamplePlugin {
    manifest: PluginManifest,
    config: serde_json::Value,
    greeting: String,
}

impl ExamplePlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "com.example.unly-example-plugin".to_string(),
                name: "Example Plugin".to_string(),
                version: "0.1.0".to_string(),
                description: "A simple example plugin that greets users.".to_string(),
                author: "Unly Project".to_string(),
                min_platform_version: "0.1.0".to_string(),
                permissions: vec!["send_message".to_string()],
                provides_tools: false,
                provides_commands: true,
                provides_jobs: true,
                enabled_by_default: true,
            },
            config: serde_json::Value::Null,
            greeting: "Hello from the example plugin!".to_string(),
        }
    }
}

impl Default for ExamplePlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for ExamplePlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn init(&mut self, config: serde_json::Value) -> Result<()> {
        info!("example plugin initialized");
        if let Some(greeting) = config.get("greeting").and_then(|v| v.as_str()) {
            self.greeting = greeting.to_string();
        }
        self.config = config;
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        info!("example plugin shutting down");
        Ok(())
    }

    fn commands(&self) -> Vec<PluginCommand> {
        vec![PluginCommand {
            name: "greet".to_string(),
            description: "Send a greeting message.".to_string(),
            usage: "/greet [name]".to_string(),
        }]
    }

    fn jobs(&self) -> Vec<PluginJob> {
        vec![PluginJob {
            name: "example_heartbeat".to_string(),
            cron_expression: "0 * * * * *".to_string(), // every minute
            description: "Example heartbeat job.".to_string(),
        }]
    }

    async fn on_event(&self, event: &PluginEvent) -> Result<()> {
        match event {
            PluginEvent::Startup => info!("example plugin: startup event"),
            PluginEvent::Shutdown => info!("example plugin: shutdown event"),
            PluginEvent::MessageReceived { chat_id, content, .. } => {
                if content.to_lowercase().contains("hello") {
                    info!(chat_id = %chat_id, "example plugin: detected greeting");
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn execute_command(&self, command: &str, args: &str, chat_id: &str) -> Result<String> {
        match command {
            "greet" => {
                let name = if args.is_empty() { "friend" } else { args };
                Ok(format!("{} Nice to meet you, {}!", self.greeting, name))
            }
            _ => Ok(format!("Unknown command: {}", command)),
        }
    }
}
