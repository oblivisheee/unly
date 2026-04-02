use async_trait::async_trait;
use std::pin::Pin;
use futures::Stream;
use crate::error::Result;
use crate::model::{ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, Model, StreamChunk};
use crate::types::{HealthReport, ProviderCapabilities};

/// Boxed token stream returned by [`Provider::chat_stream`].
pub type TokenStream = Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>;

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

    /// Send a chat completion request (non-streaming).
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse>;

    /// Stream a chat completion response token by token.
    ///
    /// Each item in the returned stream is either a [`StreamChunk::Delta`]
    /// containing a new text token, or a final [`StreamChunk::Done`] with
    /// the full response.
    ///
    /// The default implementation falls back to [`Self::chat`] and emits a
    /// single `Done` chunk (no intermediate deltas).
    async fn chat_stream(&self, request: ChatRequest) -> Result<TokenStream> {
        use futures::stream;
        let resp = self.chat(request).await?;
        let chunks: Vec<Result<StreamChunk>> = vec![Ok(StreamChunk::Done(resp))];
        Ok(Box::pin(stream::iter(chunks)))
    }

    /// Generate embeddings.
    async fn embeddings(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse>;

    /// Health check.
    async fn health(&self) -> HealthReport;
}
