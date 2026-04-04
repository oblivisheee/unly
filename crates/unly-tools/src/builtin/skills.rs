//! Self-configuration tools: manage skills and plugins from within the agent.
//!
//! These tools allow the agent to list, create, enable, disable, and remove
//! skills and plugins at runtime.  They operate on the filesystem-backed
//! skill/plugin directories and are hot-reloaded on the very next turn.
//!
//! # Risk classification
//! All management tools are `Privileged` — they modify agent behaviour but
//! are not destructive in an irreversible way.

use std::path::PathBuf;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{Value, json};

use unly_core::{
    Result,
    tool::{Tool, ToolContext, ToolResult, ToolRisk, ToolSchema},
};
use unly_plugins::{PluginLoader, SkillLoader, build_frontmatter};

// ── Shared helper ─────────────────────────────────────────────────────────────

/// Build the full SKILL.md / PLUGIN.md file content from the provided args.
///
/// Expects `args` to contain `instructions` (required), and optionally
/// `description`, `version`, and `author`.
fn build_md_content(name: &str, args: &Value) -> String {
    let instructions = args
        .get("instructions")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let version = args
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.1.0")
        .to_string();
    let author = args
        .get("author")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let frontmatter = build_frontmatter(name, &description, &version, &author);
    format!("{}\n{}\n", frontmatter, instructions)
}

// ── skill_list ────────────────────────────────────────────────────────────────

/// List all installed skills and their enabled/disabled status.
pub struct SkillListTool {
    pub skills_dir: PathBuf,
}

#[async_trait]
impl Tool for SkillListTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "skill_list".to_string(),
            description: "List all installed skills with their name, status (enabled/disabled), description, and path.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            risk: ToolRisk::Safe,
            requires_approval: false,
        }
    }

    async fn execute(&self, _args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let skills = SkillLoader::load_from_dir(&self.skills_dir);
        if skills.is_empty() {
            return Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                "No skills installed.",
                start.elapsed().as_millis() as u64,
            ));
        }
        let lines: Vec<String> = skills
            .iter()
            .map(|s| {
                let status = if s.enabled { "enabled" } else { "disabled" };
                format!(
                    "- {} [{}] — {} (path: {})",
                    s.meta.name,
                    status,
                    if s.meta.description.is_empty() {
                        "(no description)"
                    } else {
                        &s.meta.description
                    },
                    s.path.display()
                )
            })
            .collect();
        Ok(ToolResult::success(
            ctx.tool_call_id.clone(),
            lines.join("\n"),
            start.elapsed().as_millis() as u64,
        ))
    }
}

// ── skill_create ──────────────────────────────────────────────────────────────

/// Create and install a new skill from provided content.
pub struct SkillCreateTool {
    pub skills_dir: PathBuf,
}

#[async_trait]
impl Tool for SkillCreateTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "skill_create".to_string(),
            description: "Create and install a new skill. Writes a SKILL.md to the skills directory and makes it available immediately on the next turn.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Unique skill identifier (lowercase, hyphens allowed, e.g. 'my-skill')."
                    },
                    "description": {
                        "type": "string",
                        "description": "Short human-readable description of the skill."
                    },
                    "instructions": {
                        "type": "string",
                        "description": "Markdown body that will be injected into the system prompt as skill instructions."
                    },
                    "version": {
                        "type": "string",
                        "description": "Optional semver version string (e.g. '0.1.0')."
                    },
                    "author": {
                        "type": "string",
                        "description": "Optional author name."
                    }
                },
                "required": ["name", "instructions"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "missing required argument: 'name'",
                    start.elapsed().as_millis() as u64,
                ));
            }
        };
        if args
            .get("instructions")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
        {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "missing required argument: 'instructions'",
                start.elapsed().as_millis() as u64,
            ));
        }

        let skill_md_content = build_md_content(&name, &args);

        // Create directory and write SKILL.md.
        let skill_dir = self.skills_dir.join(&name);
        if skill_dir.exists() {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!(
                    "skill '{}' already exists at '{}'. Remove it first with skill_remove.",
                    name,
                    skill_dir.display()
                ),
                start.elapsed().as_millis() as u64,
            ));
        }
        if let Err(e) = std::fs::create_dir_all(&skill_dir) {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("failed to create skill directory: {}", e),
                start.elapsed().as_millis() as u64,
            ));
        }
        if let Err(e) = std::fs::write(skill_dir.join("SKILL.md"), &skill_md_content) {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("failed to write SKILL.md: {}", e),
                start.elapsed().as_millis() as u64,
            ));
        }
        Ok(ToolResult::success(
            ctx.tool_call_id.clone(),
            format!(
                "Skill '{}' created at '{}'. It will be active on the next turn.",
                name,
                skill_dir.display()
            ),
            start.elapsed().as_millis() as u64,
        ))
    }
}

// ── skill_enable ──────────────────────────────────────────────────────────────

/// Enable a previously disabled skill.
pub struct SkillEnableTool {
    pub skills_dir: PathBuf,
}

#[async_trait]
impl Tool for SkillEnableTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "skill_enable".to_string(),
            description: "Enable a previously disabled skill by its directory name.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Skill directory name." }
                },
                "required": ["id"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "missing required argument: 'id'",
                    start.elapsed().as_millis() as u64,
                ));
            }
        };
        match SkillLoader::enable(&id, &self.skills_dir) {
            Ok(()) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!("Skill '{}' enabled.", id),
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e,
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

// ── skill_disable ─────────────────────────────────────────────────────────────

/// Disable a skill (keeps it installed but inactive).
pub struct SkillDisableTool {
    pub skills_dir: PathBuf,
}

#[async_trait]
impl Tool for SkillDisableTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "skill_disable".to_string(),
            description: "Disable a skill by its directory name. The skill stays installed but will not be injected into the system prompt.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Skill directory name." }
                },
                "required": ["id"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "missing required argument: 'id'",
                    start.elapsed().as_millis() as u64,
                ));
            }
        };
        match SkillLoader::disable(&id, &self.skills_dir) {
            Ok(()) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!("Skill '{}' disabled.", id),
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e,
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

// ── skill_remove ──────────────────────────────────────────────────────────────

/// Remove an installed skill.
pub struct SkillRemoveTool {
    pub skills_dir: PathBuf,
}

#[async_trait]
impl Tool for SkillRemoveTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "skill_remove".to_string(),
            description: "Permanently remove a skill by its directory name.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Skill directory name." }
                },
                "required": ["id"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "missing required argument: 'id'",
                    start.elapsed().as_millis() as u64,
                ));
            }
        };
        match SkillLoader::remove(&id, &self.skills_dir) {
            Ok(()) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!("Skill '{}' removed.", id),
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e,
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

// ── plugin_list ───────────────────────────────────────────────────────────────

/// List all installed file-based plugins and their status.
pub struct PluginListTool {
    pub plugins_dir: PathBuf,
}

#[async_trait]
impl Tool for PluginListTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plugin_list".to_string(),
            description: "List all installed plugins (PLUGIN.md-based) with their name, status (enabled/disabled), description, and path.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            risk: ToolRisk::Safe,
            requires_approval: false,
        }
    }

    async fn execute(&self, _args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let plugins = PluginLoader::load_from_dir(&self.plugins_dir);
        if plugins.is_empty() {
            return Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                "No plugins installed.",
                start.elapsed().as_millis() as u64,
            ));
        }
        let lines: Vec<String> = plugins
            .iter()
            .map(|p| {
                let status = if p.enabled { "enabled" } else { "disabled" };
                format!(
                    "- {} [{}] — {} (path: {})",
                    p.meta.name,
                    status,
                    if p.meta.description.is_empty() {
                        "(no description)"
                    } else {
                        &p.meta.description
                    },
                    p.path.display()
                )
            })
            .collect();
        Ok(ToolResult::success(
            ctx.tool_call_id.clone(),
            lines.join("\n"),
            start.elapsed().as_millis() as u64,
        ))
    }
}

// ── plugin_create ─────────────────────────────────────────────────────────────

/// Create and install a new file-based plugin.
pub struct PluginCreateTool {
    pub plugins_dir: PathBuf,
}

#[async_trait]
impl Tool for PluginCreateTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plugin_create".to_string(),
            description: "Create and install a new plugin. Writes a PLUGIN.md to the plugins directory. The plugin instructions are injected into the system prompt on the next turn.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Unique plugin identifier (lowercase, hyphens allowed)."
                    },
                    "description": {
                        "type": "string",
                        "description": "Short human-readable description."
                    },
                    "instructions": {
                        "type": "string",
                        "description": "Markdown body with plugin instructions to inject into the system prompt."
                    },
                    "version": {
                        "type": "string",
                        "description": "Optional semver version string."
                    },
                    "author": {
                        "type": "string",
                        "description": "Optional author name."
                    }
                },
                "required": ["name", "instructions"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let name = match args.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "missing required argument: 'name'",
                    start.elapsed().as_millis() as u64,
                ));
            }
        };
        if args
            .get("instructions")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
        {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                "missing required argument: 'instructions'",
                start.elapsed().as_millis() as u64,
            ));
        }

        let plugin_md_content = build_md_content(&name, &args);

        let plugin_dir = self.plugins_dir.join(&name);
        if plugin_dir.exists() {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!(
                    "plugin '{}' already exists at '{}'. Remove it first with plugin_remove.",
                    name,
                    plugin_dir.display()
                ),
                start.elapsed().as_millis() as u64,
            ));
        }
        if let Err(e) = std::fs::create_dir_all(&plugin_dir) {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("failed to create plugin directory: {}", e),
                start.elapsed().as_millis() as u64,
            ));
        }
        if let Err(e) = std::fs::write(plugin_dir.join("PLUGIN.md"), &plugin_md_content) {
            return Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                format!("failed to write PLUGIN.md: {}", e),
                start.elapsed().as_millis() as u64,
            ));
        }
        Ok(ToolResult::success(
            ctx.tool_call_id.clone(),
            format!(
                "Plugin '{}' created at '{}'. It will be active on the next turn.",
                name,
                plugin_dir.display()
            ),
            start.elapsed().as_millis() as u64,
        ))
    }
}

// ── plugin_enable ─────────────────────────────────────────────────────────────

/// Enable a previously disabled plugin.
pub struct PluginEnableTool {
    pub plugins_dir: PathBuf,
}

#[async_trait]
impl Tool for PluginEnableTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plugin_enable".to_string(),
            description: "Enable a previously disabled plugin by its directory name.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Plugin directory name." }
                },
                "required": ["id"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "missing required argument: 'id'",
                    start.elapsed().as_millis() as u64,
                ));
            }
        };
        match PluginLoader::enable(&id, &self.plugins_dir) {
            Ok(()) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!("Plugin '{}' enabled.", id),
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e,
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

// ── plugin_disable ────────────────────────────────────────────────────────────

/// Disable a plugin (keeps it installed but inactive).
pub struct PluginDisableTool {
    pub plugins_dir: PathBuf,
}

#[async_trait]
impl Tool for PluginDisableTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plugin_disable".to_string(),
            description: "Disable a plugin by its directory name. The plugin stays installed but will not be injected into the system prompt.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Plugin directory name." }
                },
                "required": ["id"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "missing required argument: 'id'",
                    start.elapsed().as_millis() as u64,
                ));
            }
        };
        match PluginLoader::disable(&id, &self.plugins_dir) {
            Ok(()) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!("Plugin '{}' disabled.", id),
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e,
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}

// ── plugin_remove ─────────────────────────────────────────────────────────────

/// Permanently remove a plugin.
pub struct PluginRemoveTool {
    pub plugins_dir: PathBuf,
}

#[async_trait]
impl Tool for PluginRemoveTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "plugin_remove".to_string(),
            description: "Permanently remove a plugin by its directory name.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Plugin directory name." }
                },
                "required": ["id"]
            }),
            risk: ToolRisk::Privileged,
            requires_approval: false,
        }
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolResult> {
        let start = Instant::now();
        let id = match args.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => {
                return Ok(ToolResult::error(
                    ctx.tool_call_id.clone(),
                    "missing required argument: 'id'",
                    start.elapsed().as_millis() as u64,
                ));
            }
        };
        match PluginLoader::remove(&id, &self.plugins_dir) {
            Ok(()) => Ok(ToolResult::success(
                ctx.tool_call_id.clone(),
                format!("Plugin '{}' removed.", id),
                start.elapsed().as_millis() as u64,
            )),
            Err(e) => Ok(ToolResult::error(
                ctx.tool_call_id.clone(),
                e,
                start.elapsed().as_millis() as u64,
            )),
        }
    }
}
