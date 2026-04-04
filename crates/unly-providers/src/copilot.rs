//! GitHub Copilot provider implementation.
//!
//! Authentication uses the GitHub OAuth Device Flow to obtain a GitHub access token,
//! which is then exchanged for a short-lived Copilot token. The Copilot API is
//! OpenAI-compatible, so the same request format is reused.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use parking_lot::RwLock;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};

use unly_core::{
    Error, Result,
    model::{
        ChatMessageContent, ChatRequest, ChatResponse, ContentPart, EmbeddingRequest,
        EmbeddingResponse, FunctionCall, Model, StreamChunk, ToolCall, Usage,
    },
    provider::{Provider, TokenStream},
    types::{HealthReport, ProviderCapabilities},
};

use crate::error::{ProviderError, ProviderResult};

const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const COPILOT_TOKENS_URL: &str = "https://api.github.com/copilot_internal/v2/token";

/// Cached GitHub OAuth token persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedToken {
    pub access_token: String,
    pub token_type: String,
    pub scope: String,
    pub saved_at: DateTime<Utc>,
}

/// A short-lived Copilot API token.
#[derive(Debug, Clone)]
struct CopilotToken {
    token: String,
    expires_at: DateTime<Utc>,
}

/// GitHub Copilot provider.
pub struct CopilotProvider {
    client: Client,
    github_client_id: String,
    token_cache_path: PathBuf,
    copilot_api_url: String,
    github_token: Arc<RwLock<Option<String>>>,
    copilot_token: Arc<RwLock<Option<CopilotToken>>>,
}

impl CopilotProvider {
    /// Create a new CopilotProvider.
    pub fn new(
        github_client_id: impl Into<String>,
        token_cache_path: impl Into<PathBuf>,
        copilot_api_url: impl Into<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent("unly-agent/0.1.0")
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            github_client_id: github_client_id.into(),
            token_cache_path: token_cache_path.into(),
            copilot_api_url: copilot_api_url.into(),
            github_token: Arc::new(RwLock::new(None)),
            copilot_token: Arc::new(RwLock::new(None)),
        }
    }

    /// Load a cached GitHub token from disk if it exists.
    pub fn load_cached_token(&self) -> Option<CachedToken> {
        if !self.token_cache_path.exists() {
            return None;
        }
        match std::fs::read_to_string(&self.token_cache_path) {
            Ok(content) => serde_json::from_str(&content).ok(),
            Err(_) => None,
        }
    }

    /// Save a GitHub token to disk.
    pub fn save_token(&self, token: &CachedToken) -> std::io::Result<()> {
        if let Some(parent) = self.token_cache_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(token).map_err(std::io::Error::other)?;
        std::fs::write(&self.token_cache_path, content)?;
        // Set restrictive file permissions on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &self.token_cache_path,
                std::fs::Permissions::from_mode(0o600),
            )?;
        }
        Ok(())
    }

    /// Initialize from a cached token on disk.
    pub fn init_from_cache(&self) -> bool {
        if let Some(cached) = self.load_cached_token() {
            info!("loaded GitHub token from cache");
            *self.github_token.write() = Some(cached.access_token);
            true
        } else {
            false
        }
    }

    /// Start the GitHub OAuth Device Flow and return user-facing instructions.
    pub async fn start_device_flow(&self) -> ProviderResult<DeviceFlowState> {
        let response = self
            .client
            .post(GITHUB_DEVICE_CODE_URL)
            .header("Accept", "application/json")
            .form(&[("client_id", &self.github_client_id)])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ProviderError::Auth(format!(
                "device flow request failed: {}",
                response.status()
            )));
        }

        let state: DeviceFlowState = response.json().await?;
        Ok(state)
    }

    /// Poll the GitHub token endpoint during the device flow.
    pub async fn poll_device_flow(
        &self,
        state: &DeviceFlowState,
    ) -> ProviderResult<DevicePollResult> {
        let response = self
            .client
            .post(GITHUB_TOKEN_URL)
            .header("Accept", "application/json")
            .form(&[
                ("client_id", self.github_client_id.as_str()),
                ("device_code", state.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await?;

        let body: serde_json::Value = response.json().await?;

        if let Some(token) = body.get("access_token").and_then(|v| v.as_str()) {
            let cached = CachedToken {
                access_token: token.to_string(),
                token_type: body
                    .get("token_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("bearer")
                    .to_string(),
                scope: body
                    .get("scope")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                saved_at: Utc::now(),
            };
            self.save_token(&cached).ok();
            *self.github_token.write() = Some(token.to_string());
            return Ok(DevicePollResult::Authorized);
        }

        if let Some(error) = body.get("error").and_then(|v| v.as_str()) {
            return Ok(match error {
                "authorization_pending" => DevicePollResult::Pending,
                "slow_down" => DevicePollResult::SlowDown,
                "expired_token" => DevicePollResult::Expired,
                "access_denied" => DevicePollResult::Denied,
                other => DevicePollResult::Error(other.to_string()),
            });
        }

        Ok(DevicePollResult::Error("unexpected response".to_string()))
    }

    /// Get a valid Copilot API token, refreshing if necessary.
    async fn get_copilot_token(&self) -> ProviderResult<String> {
        // Check if the existing token is still valid.
        {
            let guard = self.copilot_token.read();
            if let Some(token) = guard.as_ref()
                && token.expires_at > Utc::now() + chrono::Duration::seconds(30)
            {
                return Ok(token.token.clone());
            }
        }

        // Need to fetch a new Copilot token using the GitHub token.
        let github_token = {
            let guard = self.github_token.read();
            guard.clone().ok_or_else(|| {
                ProviderError::Auth(
                    "not authenticated — run `unly provider login copilot`".to_string(),
                )
            })?
        };

        debug!("fetching new Copilot API token");

        let response = self
            .client
            .get(COPILOT_TOKENS_URL)
            .header("Authorization", format!("token {}", github_token))
            .header("Accept", "application/json")
            .header("Editor-Version", "unly/0.1.0")
            .header("Editor-Plugin-Version", "unly-agent/0.1.0")
            .send()
            .await?;

        if response.status() == 401 {
            return Err(ProviderError::Auth(
                "GitHub token rejected — please re-authenticate".to_string(),
            ));
        }

        if !response.status().is_success() {
            return Err(ProviderError::RequestFailed {
                status: response.status().as_u16(),
                message: "failed to obtain Copilot token".to_string(),
            });
        }

        let body: serde_json::Value = response.json().await?;
        let token = body
            .get("token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::Auth("no token in Copilot response".to_string()))?
            .to_string();

        // Parse expiry. Format: Unix timestamp in body["expires_at"].
        let expires_at = body
            .get("expires_at")
            .and_then(|v| v.as_i64())
            .map(|ts| {
                DateTime::from_timestamp(ts, 0)
                    .unwrap_or_else(|| Utc::now() + chrono::Duration::minutes(25))
            })
            .unwrap_or_else(|| Utc::now() + chrono::Duration::minutes(25));

        *self.copilot_token.write() = Some(CopilotToken {
            token: token.clone(),
            expires_at,
        });

        Ok(token)
    }

    /// Build the OpenAI-compatible chat completions request body.
    fn build_chat_body(request: &ChatRequest) -> serde_json::Value {
        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| {
                let mut obj = serde_json::json!({
                    "role": m.role,
                    "content": match &m.content {
                        ChatMessageContent::Text(s) => serde_json::Value::String(s.clone()),
                        ChatMessageContent::Parts(parts) => {
                            serde_json::Value::Array(
                                parts.iter().map(|p| match p {
                                    ContentPart::Text { text } => serde_json::json!({"type": "text", "text": text}),
                                    ContentPart::ImageUrl { image_url } => serde_json::json!({"type": "image_url", "image_url": {"url": image_url.url}}),
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
            body["temperature"] = serde_json::Value::Number(
                serde_json::Number::from_f64(temp as f64).unwrap_or(serde_json::Number::from(1)),
            );
        }
        if let Some(max) = request.max_tokens {
            body["max_tokens"] = serde_json::Value::Number(max.into());
        }
        if let Some(tools) = &request.tools {
            body["tools"] = serde_json::to_value(tools).unwrap_or_default();
        }

        body
    }
}

/// State returned from starting the GitHub Device Flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceFlowState {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Result of polling the device flow endpoint.
#[derive(Debug, Clone)]
pub enum DevicePollResult {
    Authorized,
    Pending,
    SlowDown,
    Expired,
    Denied,
    Error(String),
}

#[async_trait]
impl Provider for CopilotProvider {
    fn name(&self) -> &str {
        "copilot"
    }

    fn description(&self) -> &str {
        "GitHub Copilot — primary LLM provider using GitHub OAuth authentication"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            chat: true,
            embeddings: true,
            tool_calling: true,
            streaming: true,
            vision: true,
            reasoning: true,
        }
    }

    async fn list_models(&self) -> Result<Vec<Model>> {
        let token = self
            .get_copilot_token()
            .await
            .map_err(|e| Error::provider("copilot", e.to_string()))?;

        let url = format!("{}/models", self.copilot_api_url);
        let response = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .header("Copilot-Integration-Id", "vscode-chat")
            .send()
            .await
            .map_err(|e| Error::provider("copilot", e.to_string()))?;

        if !response.status().is_success() {
            // Fall back to known Copilot models if listing fails.
            return Ok(known_copilot_models());
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::provider("copilot", e.to_string()))?;

        let models = parse_openai_models(&body, "copilot");
        Ok(if models.is_empty() {
            known_copilot_models()
        } else {
            models
        })
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let token = self
            .get_copilot_token()
            .await
            .map_err(|e| Error::provider("copilot", e.to_string()))?;

        let url = format!("{}/chat/completions", self.copilot_api_url);
        let body = Self::build_chat_body(&request);

        let response = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .header("Copilot-Integration-Id", "vscode-chat")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::provider("copilot", e.to_string()))?;

        if response.status() == 401 {
            return Err(Error::Auth(
                "Copilot token rejected — please re-authenticate".to_string(),
            ));
        }

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(Error::provider(
                "copilot",
                format!("HTTP {} — {}", status, text),
            ));
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::provider("copilot", e.to_string()))?;

        parse_chat_response(body).map_err(|e| Error::provider("copilot", e))
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<TokenStream> {
        let token = self
            .get_copilot_token()
            .await
            .map_err(|e| Error::provider("copilot", e.to_string()))?;

        let url = format!("{}/chat/completions", self.copilot_api_url);
        // Build body with stream: true (override whatever the caller set).
        let mut body = Self::build_chat_body(&request);
        body["stream"] = serde_json::json!(true);
        let model_name = request.model.clone();

        let response = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .header("Copilot-Integration-Id", "vscode-chat")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::provider("copilot", e.to_string()))?;

        if response.status() == 401 {
            return Err(Error::Auth(
                "Copilot token rejected — please re-authenticate".to_string(),
            ));
        }

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(Error::provider(
                "copilot",
                format!("HTTP {} — {}", status, text),
            ));
        }

        let byte_stream = response.bytes_stream();

        let stream = futures::stream::unfold(
            (byte_stream, String::new(), String::new(), model_name),
            |(mut byte_stream, mut buf, mut accumulated, mname)| async move {
                loop {
                    if let Some(pos) = buf.find('\n') {
                        let line = buf[..pos].trim_end_matches('\r').to_string();
                        buf = buf[pos + 1..].to_string();

                        if let Some(data) = line.strip_prefix("data: ") {
                            let data = data.trim();
                            if data == "[DONE]" {
                                let resp = ChatResponse {
                                    id: String::new(),
                                    model: mname.clone(),
                                    content: Some(accumulated.clone()),
                                    tool_calls: None,
                                    finish_reason: Some("stop".to_string()),
                                    usage: None,
                                };
                                return Some((
                                    Ok(StreamChunk::Done(resp)),
                                    (byte_stream, buf, accumulated, mname),
                                ));
                            }

                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                let delta = json
                                    .pointer("/choices/0/delta/content")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                if !delta.is_empty() {
                                    accumulated.push_str(&delta);
                                    return Some((
                                        Ok(StreamChunk::Delta(delta)),
                                        (byte_stream, buf, accumulated, mname),
                                    ));
                                }
                            }
                        }
                        continue;
                    }

                    match byte_stream.next().await {
                        Some(Ok(bytes)) => {
                            buf.push_str(&String::from_utf8_lossy(&bytes));
                        }
                        Some(Err(e)) => {
                            return Some((
                                Err(Error::provider("copilot", e.to_string())),
                                (byte_stream, buf, accumulated, mname),
                            ));
                        }
                        None => {
                            if !accumulated.is_empty() {
                                let resp = ChatResponse {
                                    id: String::new(),
                                    model: mname.clone(),
                                    content: Some(accumulated.clone()),
                                    tool_calls: None,
                                    finish_reason: Some("stop".to_string()),
                                    usage: None,
                                };
                                return Some((
                                    Ok(StreamChunk::Done(resp)),
                                    (byte_stream, buf, accumulated, mname),
                                ));
                            }
                            return None;
                        }
                    }
                }
            },
        );

        Ok(Box::pin(stream))
    }

    async fn embeddings(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse> {
        let token = self
            .get_copilot_token()
            .await
            .map_err(|e| Error::provider("copilot", e.to_string()))?;

        let url = format!("{}/embeddings", self.copilot_api_url);
        let body = serde_json::json!({
            "model": request.model,
            "input": request.input,
        });

        let response = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .header("Copilot-Integration-Id", "vscode-chat")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::provider("copilot", e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(Error::provider(
                "copilot",
                format!("embeddings HTTP {} — {}", status, text),
            ));
        }

        let resp_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::provider("copilot", e.to_string()))?;

        parse_embeddings_response(resp_body, &request.model)
            .map_err(|e| Error::provider("copilot", e))
    }

    async fn health(&self) -> HealthReport {
        match self.get_copilot_token().await {
            Ok(_) => HealthReport::healthy("copilot"),
            Err(e) => HealthReport::unhealthy("copilot", e.to_string()),
        }
    }
}

fn known_copilot_models() -> Vec<Model> {
    vec![
        Model {
            id: "gpt-4o".to_string(),
            name: "GPT-4o".to_string(),
            provider: "copilot".to_string(),
            context_window: Some(128_000),
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
        },
        Model {
            id: "gpt-4o-mini".to_string(),
            name: "GPT-4o Mini".to_string(),
            provider: "copilot".to_string(),
            context_window: Some(128_000),
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
        },
        Model {
            id: "claude-3-5-sonnet".to_string(),
            name: "Claude 3.5 Sonnet".to_string(),
            provider: "copilot".to_string(),
            context_window: Some(200_000),
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
        },
        Model {
            id: "o1-preview".to_string(),
            name: "o1-preview".to_string(),
            provider: "copilot".to_string(),
            context_window: Some(128_000),
            supports_vision: false,
            supports_tools: false,
            supports_streaming: false,
        },
        Model {
            id: "o3-mini".to_string(),
            name: "o3-mini".to_string(),
            provider: "copilot".to_string(),
            context_window: Some(200_000),
            supports_vision: false,
            supports_tools: true,
            supports_streaming: false,
        },
    ]
}

pub(crate) fn parse_openai_models(body: &serde_json::Value, provider: &str) -> Vec<Model> {
    body.get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(|v| v.as_str())?.to_string();
                    Some(Model {
                        id: id.clone(),
                        name: id,
                        provider: provider.to_string(),
                        context_window: None,
                        supports_vision: false,
                        supports_tools: true,
                        supports_streaming: true,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn parse_chat_response(
    body: serde_json::Value,
) -> std::result::Result<ChatResponse, String> {
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let choice = body
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .ok_or_else(|| "no choices in response".to_string())?;

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let message = choice
        .get("message")
        .ok_or_else(|| "no message in choice".to_string())?;

    let content = message
        .get("content")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let tool_calls = message
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|tc| {
                    let id = tc.get("id").and_then(|v| v.as_str())?.to_string();
                    let typ = tc
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("function")
                        .to_string();
                    let func = tc.get("function")?;
                    let name = func.get("name").and_then(|v| v.as_str())?.to_string();
                    let arguments = func
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}")
                        .to_string();
                    Some(ToolCall {
                        id,
                        r#type: typ,
                        function: FunctionCall { name, arguments },
                    })
                })
                .collect::<Vec<_>>()
        });

    let usage = body.get("usage").map(|u| Usage {
        prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        completion_tokens: u
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
    });

    Ok(ChatResponse {
        id,
        model,
        content,
        tool_calls,
        finish_reason,
        usage,
    })
}

pub(crate) fn parse_embeddings_response(
    body: serde_json::Value,
    model: &str,
) -> std::result::Result<EmbeddingResponse, String> {
    let data = body
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| "no data in embeddings response".to_string())?;

    let mut embeddings = Vec::new();
    for item in data {
        let embedding = item
            .get("embedding")
            .and_then(|e| e.as_array())
            .ok_or_else(|| "no embedding in data item".to_string())?;
        let vec: Vec<f32> = embedding
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect();
        embeddings.push(vec);
    }

    let usage = body.get("usage").map(|u| Usage {
        prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        completion_tokens: 0,
        total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
    });

    Ok(EmbeddingResponse {
        embeddings,
        model: model.to_string(),
        usage,
    })
}
