//! Tests for the plugin system.

use async_trait::async_trait;
use unly_core::{tool::ToolSchema, Result};
use unly_plugins::{
    manifest::PluginManifest,
    plugin::{Plugin, PluginEvent},
    PluginRegistry,
};

struct MinimalPlugin {
    manifest: PluginManifest,
}

impl MinimalPlugin {
    fn new(id: &str) -> Self {
        Self {
            manifest: PluginManifest {
                id: id.to_string(),
                name: format!("Test Plugin {}", id),
                version: "0.1.0".to_string(),
                min_platform_version: "0.1.0".to_string(),
                ..Default::default()
            },
        }
    }
}

#[async_trait]
impl Plugin for MinimalPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn init(&mut self, _config: serde_json::Value) -> Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn register_and_list_plugins() {
    let registry = PluginRegistry::new(vec![], vec![]);
    registry
        .register(Box::new(MinimalPlugin::new("com.test.plugin-a")))
        .await
        .expect("registration should succeed");
    registry
        .register(Box::new(MinimalPlugin::new("com.test.plugin-b")))
        .await
        .expect("registration should succeed");

    let mut names = registry.list().await;
    names.sort();
    assert_eq!(names, vec!["com.test.plugin-a", "com.test.plugin-b"]);
}

#[tokio::test]
async fn disabled_plugin_is_not_registered() {
    let registry = PluginRegistry::new(vec![], vec!["com.test.blocked".to_string()]);
    registry
        .register(Box::new(MinimalPlugin::new("com.test.blocked")))
        .await
        .expect("should not error when skipping a disabled plugin");

    assert!(
        registry.list().await.is_empty(),
        "disabled plugin should not appear in list"
    );
}

#[tokio::test]
async fn incompatible_plugin_is_rejected() {
    let registry = PluginRegistry::new(vec![], vec![]);

    let mut plugin = MinimalPlugin::new("com.test.future");
    plugin.manifest.min_platform_version = "99.0.0".to_string();

    let result = registry.register(Box::new(plugin)).await;
    assert!(result.is_err(), "future-version plugin should be rejected");
}
