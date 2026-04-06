//! Self-configuration tools for runtime config.toml.

use std::time::Instant;

use async_trait::async_trait;
use serde_json::{Value, json};

use unly_core::{
    Result,
    tool::{Tool, ToolContext, ToolResult, ToolRisk, ToolSchema},
};

pub struct SelfConfigGetTool;

#[async_trait]
impl Tool for SelfConfigGetTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "self_config_get".to_string(),
            description: "Read unly config.toml. Optionally read one dotted key (example: `tools.require_approval_for_privileged`).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Optional dotted key path." }
                },
                "required": []
            }),
            risk: ToolRisk::Safe,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let key = args.get("key").and_then(|v| v.as_str()).map(str::trim);
        let path = unly_config::workspace::default_config_path();
        let raw = match std::fs::read_to_string(&path) {
            Ok(v) => v,
            Err(e) => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    format!("failed to read config '{}': {}", path.display(), e),
                    start.elapsed().as_millis() as u64,
                ));
            }
        };
        let parsed: toml::Value = match toml::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    format!("failed to parse config TOML: {}", e),
                    start.elapsed().as_millis() as u64,
                ));
            }
        };

        let output = if let Some(k) = key.filter(|k| !k.is_empty()) {
            let Some(v) = get_dotted(&parsed, k) else {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    format!("config key not found: {}", k),
                    start.elapsed().as_millis() as u64,
                ));
            };
            json!({
                "path": path,
                "key": k,
                "value": serde_json::to_value(v).unwrap_or(Value::Null),
            })
        } else {
            json!({
                "path": path,
                "value": serde_json::to_value(parsed).unwrap_or(Value::Null),
            })
        };

        Ok(ToolResult::success(
            ctx.tool_call_id.clone(),
            output.to_string(),
            start.elapsed().as_millis() as u64,
        ))
    }
}

pub struct SelfConfigSetTool;

#[async_trait]
impl Tool for SelfConfigSetTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "self_config_set".to_string(),
            description: "Set one config.toml key by dotted path and persist it. Example key: `agent.max_tool_calls_per_turn`.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Dotted key path to set." },
                    "value": { "description": "New value (string/number/bool/array/object)." }
                },
                "required": ["key", "value"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let key = match args.get("key").and_then(|v| v.as_str()).map(str::trim) {
            Some(k) if !k.is_empty() => k.to_string(),
            _ => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "missing required argument: 'key'",
                    start.elapsed().as_millis() as u64,
                ));
            }
        };
        let Some(json_value) = args.get("value") else {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "missing required argument: 'value'",
                start.elapsed().as_millis() as u64,
            ));
        };

        let path = unly_config::workspace::default_config_path();
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        let mut parsed: toml::Value = if raw.trim().is_empty() {
            toml::Value::Table(Default::default())
        } else {
            match toml::from_str(&raw) {
                Ok(v) => v,
                Err(e) => {
                    return Ok(ToolResult::error(
                        ctx.tool_call_id.clone(),
                        format!("failed to parse existing config TOML: {}", e),
                        start.elapsed().as_millis() as u64,
                    ));
                }
            }
        };

        let value = match json_to_toml(json_value) {
            Some(v) => v,
            None => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "unsupported value type for TOML conversion",
                    start.elapsed().as_millis() as u64,
                ));
            }
        };

        if let Err(e) = set_dotted(&mut parsed, &key, value) {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e,
                start.elapsed().as_millis() as u64,
            ));
        }

        let output = match toml::to_string_pretty(&parsed) {
            Ok(v) => v,
            Err(e) => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    format!("failed to serialize config TOML: {}", e),
                    start.elapsed().as_millis() as u64,
                ));
            }
        };
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("failed to create config directory: {}", e),
                start.elapsed().as_millis() as u64,
            ));
        }
        if let Err(e) = std::fs::write(&path, output) {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("failed to write config '{}': {}", path.display(), e),
                start.elapsed().as_millis() as u64,
            ));
        }

        Ok(ToolResult::success(
            ctx.tool_call_id.clone(),
            format!("updated config key '{}' in '{}'", key, path.display()),
            start.elapsed().as_millis() as u64,
        ))
    }
}

fn get_dotted<'a>(root: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut current = root;
    for part in key.split('.').filter(|p| !p.is_empty()) {
        current = current.get(part)?;
    }
    Some(current)
}

fn set_dotted(
    root: &mut toml::Value,
    key: &str,
    value: toml::Value,
) -> std::result::Result<(), String> {
    let parts: Vec<&str> = key.split('.').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err("key must not be empty".to_string());
    }
    if !root.is_table() {
        *root = toml::Value::Table(Default::default());
    }
    let mut current = root
        .as_table_mut()
        .ok_or_else(|| "config root is not a table".to_string())?;

    for part in &parts[..parts.len() - 1] {
        if !current.contains_key(*part) {
            current.insert((*part).to_string(), toml::Value::Table(Default::default()));
        }
        let Some(next) = current.get_mut(*part) else {
            return Err(format!("failed to access key segment '{}'", part));
        };
        if !next.is_table() {
            return Err(format!("key segment '{}' is not a table", part));
        }
        current = next
            .as_table_mut()
            .ok_or_else(|| format!("failed to access table '{}'", part))?;
    }

    current.insert(parts[parts.len() - 1].to_string(), value);
    Ok(())
}

fn json_to_toml(value: &Value) -> Option<toml::Value> {
    match value {
        Value::Null => None,
        Value::Bool(v) => Some(toml::Value::Boolean(*v)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(toml::Value::Integer(i))
            } else {
                n.as_f64().map(toml::Value::Float)
            }
        }
        Value::String(s) => Some(toml::Value::String(s.clone())),
        Value::Array(arr) => {
            let mut out = Vec::new();
            for item in arr {
                out.push(json_to_toml(item)?);
            }
            Some(toml::Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = toml::map::Map::new();
            for (k, v) in map {
                out.insert(k.clone(), json_to_toml(v)?);
            }
            Some(toml::Value::Table(out))
        }
    }
}
