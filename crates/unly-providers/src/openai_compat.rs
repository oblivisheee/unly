//! OpenAI-compatible provider implementation.
//!
//! Supports any API that follows the OpenAI REST API format, including
//! OpenAI directly, Azure OpenAI, Ollama, LM Studio, Together AI, etc.

use async_trait::async_trait;
use reqwest::Client;
use std::time::Duration;
use tracing::debug;

use unly_core::{
    model::{ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, Model},
    provider::Provider,
    types::{HealthReport, ProviderCapabilities},
    Error, Result,
};

use crate::copilot::{parse_chat_response, parse_embeddings_response, parse_openai_models};

/// OpenAI-compatible provider.
pub struct OpenAiCompatProvider {
    name: String,
    base_url: String,
    api_key: String,
    client: Client,
    static_models: Vec<String>,
}

impl OpenAiCompatProvider {
    /// Create a new OpenAI-compatible provider.
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        static_models: Vec<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent("unly-agent/0.1.0")
            .build()
            .expect("failed to build reqwest client");

        let mut base_url: String = base_url.into();
        // Normalize: remove trailing slash.
        if base_url.ends_with('/') {
            base_url.pop();
        }

        Self {
            name: name.into(),
            base_url,
            api_key: api_key.into(),
            client,
            static_models,
        }
    }

    fn build_chat_body(&self, request: &ChatRequest) -> serde_json::Value {
        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| {
                let mut obj = serde_json::json!({
                    "role": m.role,
                    "content": match &m.content {
                        unly_core::model::ChatMessageContent::Text(s) => serde_json::Value::String(s.clone()),
                        unly_core::model::ChatMessageContent::Parts(parts) => {
                            serde_json::Value::Array(
                                parts.iter().map(|p| match p {
                                    unly_core::model::ContentPart::Text { text } => serde_json::json!({"type": "text", "text": text}),
                                    unly_core::model::ContentPart::ImageUrl { image_url } => serde_json::json!({"type": "image_url", "image_url": {"url": image_url.url}}),
                                }).collect()
                            )
                        }
                    }
                });
                if let Some(id) = &m.tool_call_id {
                    obj["tool_call_id"] = serde_json::Value::String(id.clone());
                }
                if let Some(name) = &m.name {
                    obj["name"] = serde_json::Value::String(name.clone());
                }
                if let Some(tool_calls) = &m.tool_calls {
                    obj["tool_calls"] = serde_json::to_value(tool_calls).unwrap_or_default();
                }
                obj
            })
            .collect();

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "stream": request.stream,
        });
        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }
        if let Some(max) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(max);
        }
        if let Some(tools) = &request.tools {
            body["tools"] = serde_json::to_value(tools).unwrap_or_default();
        }
        body
    }
}

#[async_trait]
impl Provider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "OpenAI-compatible provider"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            chat: true,
            embeddings: true,
            tool_calling: true,
            streaming: false,
            vision: false,
        }
    }

    async fn list_models(&self) -> Result<Vec<Model>> {
        if !self.static_models.is_empty() {
            return Ok(self
                .static_models
                .iter()
                .map(|id| Model {
                    id: id.clone(),
                    name: id.clone(),
                    provider: self.name.clone(),
                    context_window: None,
                    supports_vision: false,
                    supports_tools: true,
                    supports_streaming: true,
                })
                .collect());
        }

        let url = format!("{}/models", self.base_url);
        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| Error::provider(&self.name, e.to_string()))?;

        if !response.status().is_success() {
            return Err(Error::provider(
                &self.name,
                format!("list_models failed: {}", response.status()),
            ));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::provider(&self.name, e.to_string()))?;

        Ok(parse_openai_models(&body, &self.name))
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = self.build_chat_body(&request);

        debug!(provider = %self.name, model = %request.model, "sending chat request");

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::provider(&self.name, e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(Error::provider(
                &self.name,
                format!("HTTP {} — {}", status, text),
            ));
        }

        let resp_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::provider(&self.name, e.to_string()))?;

        parse_chat_response(resp_body).map_err(|e| Error::provider(&self.name, e))
    }

    async fn embeddings(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse> {
        let url = format!("{}/embeddings", self.base_url);
        let body = serde_json::json!({
            "model": request.model,
            "input": request.input,
        });

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::provider(&self.name, e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(Error::provider(
                &self.name,
                format!("embeddings HTTP {} — {}", status, text),
            ));
        }

        let resp_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::provider(&self.name, e.to_string()))?;

        parse_embeddings_response(resp_body, &request.model)
            .map_err(|e| Error::provider(&self.name, e))
    }

    async fn health(&self) -> HealthReport {
        let url = format!("{}/models", self.base_url);
        match self
            .client
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => HealthReport::healthy(&self.name),
            Ok(r) => HealthReport::degraded(&self.name, format!("models endpoint: {}", r.status())),
            Err(e) => HealthReport::unhealthy(&self.name, e.to_string()),
        }
    }
}
