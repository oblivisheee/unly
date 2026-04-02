use async_trait::async_trait;
use crate::error::Result;
use crate::model::{ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, Model};
use crate::types::{HealthReport, ProviderCapabilities};

/// A named LLM provider.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Provider identifier (e.g. "copilot", "openai").
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// Capabilities of this provider.
    fn capabilities(&self) -> ProviderCapabilities;

    /// List available models.
    async fn list_models(&self) -> Result<Vec<Model>>;

    /// Send a chat completion request.
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse>;

    /// Generate embeddings.
    async fn embeddings(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse>;

    /// Health check.
    async fn health(&self) -> HealthReport;
}
