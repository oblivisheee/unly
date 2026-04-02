use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use unly_core::Result;

use crate::{
    plugin::{Plugin, PluginEvent},
};

const PLATFORM_VERSION: &str = "0.1.0";

/// Registry for all installed plugins.
pub struct PluginRegistry {
    plugins: Arc<RwLock<HashMap<String, Box<dyn Plugin>>>>,
    #[allow(dead_code)]
    enabled: Vec<String>,
    disabled: Vec<String>,
}

impl PluginRegistry {
    pub fn new(enabled: Vec<String>, disabled: Vec<String>) -> Self {
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            enabled,
            disabled,
        }
    }

    /// Register a plugin (takes ownership). Validates compatibility.
    pub async fn register(&self, plugin: Box<dyn Plugin>) -> Result<()> {
        let manifest = plugin.manifest();
        let id = manifest.id.clone();

        // Check if explicitly disabled.
        if self.disabled.contains(&id) {
            info!("plugin {} is disabled by configuration", id);
            return Ok(());
        }

        // Validate platform version compatibility.
        // Simple semver prefix check: compare major.minor.
        if !is_compatible(&manifest.min_platform_version, PLATFORM_VERSION) {
            return Err(unly_core::Error::Plugin {
                plugin: id.clone(),
                message: format!(
                    "requires platform >= {}, current = {}",
                    manifest.min_platform_version, PLATFORM_VERSION
                ),
            });
        }

        info!("registering plugin: {} v{}", manifest.name, manifest.version);
        self.plugins.write().await.insert(id, plugin);
        Ok(())
    }

    /// Initialize all registered plugins.
    pub async fn init_all(&self, configs: &HashMap<String, serde_json::Value>) -> Result<()> {
        let mut plugins = self.plugins.write().await;
        for (id, plugin) in plugins.iter_mut() {
            let config = configs
                .get(id)
                .cloned()
                .unwrap_or(serde_json::Value::Object(Default::default()));
            if let Err(e) = plugin.init(config).await {
                warn!("plugin {} init failed: {}", id, e);
            } else {
                info!("plugin {} initialized", id);
            }
        }
        Ok(())
    }

    /// Shutdown all plugins.
    pub async fn shutdown_all(&self) {
        let plugins = self.plugins.read().await;
        for (id, plugin) in plugins.iter() {
            if let Err(e) = plugin.shutdown().await {
                warn!("plugin {} shutdown error: {}", id, e);
            }
        }
    }

    /// Dispatch an event to all plugins.
    pub async fn dispatch(&self, event: &PluginEvent) {
        let plugins = self.plugins.read().await;
        for (id, plugin) in plugins.iter() {
            if let Err(e) = plugin.on_event(event).await {
                warn!("plugin {} event error: {}", id, e);
            }
        }
    }

    /// List all registered plugin names.
    pub async fn list(&self) -> Vec<String> {
        self.plugins.read().await.keys().cloned().collect()
    }
}

fn is_compatible(required: &str, current: &str) -> bool {
    // Simple check: current version >= required. Parse semver components.
    let parse = |v: &str| -> (u32, u32, u32) {
        let parts: Vec<u32> = v
            .split('.')
            .take(3)
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };

    let (rma, rmi, rp) = parse(required);
    let (cma, cmi, cp) = parse(current);

    (cma, cmi, cp) >= (rma, rmi, rp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compatibility() {
        assert!(is_compatible("0.1.0", "0.1.0"));
        assert!(is_compatible("0.1.0", "0.2.0"));
        assert!(!is_compatible("0.2.0", "0.1.0"));
    }
}
