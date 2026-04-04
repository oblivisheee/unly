use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

use unly_core::{
    Result,
    tool::{Tool, ToolContext, ToolResult, ToolRisk, ToolSchema},
};

/// Tool: HTTP GET request.
pub struct HttpGetTool {
    client: Client,
}

impl HttpGetTool {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("unly-agent/0.1.0")
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }
}

impl Default for HttpGetTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HttpGetTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "http_get".to_string(),
            description: "Make an HTTP GET request and return the response body.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "The URL to fetch."},
                    "headers": {
                        "type": "object",
                        "description": "Optional HTTP headers to include.",
                        "additionalProperties": {"type": "string"}
                    }
                },
                "required": ["url"]
            }),
            risk: ToolRisk::Safe,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let url = args["url"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing url argument".to_string()))?;

        // Validate URL scheme.
        if !url.starts_with("https://") && !url.starts_with("http://") {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "only http/https URLs are supported",
                start.elapsed().as_millis() as u64,
            ));
        }

        let mut req = self.client.get(url);
        if let Some(headers) = args["headers"].as_object() {
            for (k, v) in headers {
                if let Some(val) = v.as_str() {
                    req = req.header(k.as_str(), val);
                }
            }
        }

        match req.send().await {
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let duration_ms = start.elapsed().as_millis() as u64;
                if status.is_success() {
                    Ok(ToolResult::success(
                        ctx.tool_call_id.clone(),
                        body,
                        duration_ms,
                    ))
                } else {
                    Ok(ToolResult::error(
                        ctx.tool_call_id.clone(),
                        format!("HTTP {}: {}", status, body),
                        duration_ms,
                    ))
                }
            }
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e.to_string(),
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

/// Tool: HTTP POST request.
pub struct HttpPostTool {
    client: Client,
}

impl HttpPostTool {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("unly-agent/0.1.0")
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }
}

impl Default for HttpPostTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HttpPostTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "http_post".to_string(),
            description: "Make an HTTP POST request with a JSON body.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "The URL to post to."},
                    "body": {"type": "object", "description": "JSON body to send."},
                    "headers": {
                        "type": "object",
                        "description": "Optional HTTP headers.",
                        "additionalProperties": {"type": "string"}
                    }
                },
                "required": ["url", "body"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let url = args["url"]
            .as_str()
            .ok_or_else(|| unly_core::Error::InvalidInput("missing url argument".to_string()))?;

        if !url.starts_with("https://") && !url.starts_with("http://") {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "only http/https URLs are supported",
                start.elapsed().as_millis() as u64,
            ));
        }

        let body = &args["body"];
        let mut req = self.client.post(url).json(body);

        if let Some(headers) = args["headers"].as_object() {
            for (k, v) in headers {
                if let Some(val) = v.as_str() {
                    req = req.header(k.as_str(), val);
                }
            }
        }

        match req.send().await {
            Ok(response) => {
                let status = response.status();
                let resp_body = response.text().await.unwrap_or_default();
                let duration_ms = start.elapsed().as_millis() as u64;
                if status.is_success() {
                    Ok(ToolResult::success(
                        ctx.tool_call_id.clone(),
                        resp_body,
                        duration_ms,
                    ))
                } else {
                    Ok(ToolResult::error(
                        ctx.tool_call_id.clone(),
                        format!("HTTP {}: {}", status, resp_body),
                        duration_ms,
                    ))
                }
            }
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e.to_string(),
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}
