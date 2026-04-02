use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

use unly_core::{
    model::{ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, Model},
    provider::Provider,
    types::{HealthReport, HealthStatus, ProviderCapabilities},
    Error, Result,
};

/// Registry that holds all configured LLM providers.
#[derive(Clone)]
pub struct ProviderRegistry {
    providers: Arc<RwLock<HashMap<String, Arc<dyn Provider>>>>,
    default_provider: Arc<RwLock<String>>,
    default_model: Arc<RwLock<String>>,
}

impl ProviderRegistry {
    pub fn new(default_provider: &str, default_model: &str) -> Self {
        Self {
            providers: Arc::new(RwLock::new(HashMap::new())),
            default_provider: Arc::new(RwLock::new(default_provider.to_string())),
            default_model: Arc::new(RwLock::new(default_model.to_string())),
        }
    }

    /// Register a provider.
    pub fn register(&self, provider: Arc<dyn Provider>) {
        let name = provider.name().to_string();
        info!("registering provider: {}", name);
        self.providers.write().insert(name, provider);
    }

    /// Get a provider by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Provider>> {
        self.providers.read().get(name).cloned()
    }

    /// Get the default provider.
    pub fn default_provider(&self) -> Result<Arc<dyn Provider>> {
        let name = self.default_provider.read().clone();
        self.get(&name)
            .ok_or_else(|| Error::ProviderNotFound(name))
    }

    /// Get the configured default model name.
    pub fn default_model(&self) -> String {
        self.default_model.read().clone()
    }

    /// List all registered provider names.
    pub fn provider_names(&self) -> Vec<String> {
        self.providers.read().keys().cloned().collect()
    }

    /// Run health checks on all providers.
    pub async fn health_all(&self) -> Vec<HealthReport> {
        let providers: Vec<Arc<dyn Provider>> =
            self.providers.read().values().cloned().collect();
        let mut reports = Vec::new();
        for p in providers {
            let report = p.health().await;
            reports.push(report);
        }
        reports
    }

    /// Set a new default provider.
    pub fn set_default_provider(&self, name: &str) -> Result<()> {
        if self.get(name).is_none() {
            return Err(Error::ProviderNotFound(name.to_string()));
        }
        *self.default_provider.write() = name.to_string();
        Ok(())
    }
}
