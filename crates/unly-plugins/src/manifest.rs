use serde::{Deserialize, Serialize};

/// Plugin manifest describing the plugin's identity, requirements, and capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin identifier (e.g. "com.example.my-plugin").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Plugin version (semver).
    pub version: String,
    /// Short description.
    pub description: String,
    /// Plugin author.
    pub author: String,
    /// Minimum platform version required (semver).
    pub min_platform_version: String,
    /// Permissions/capabilities requested.
    pub permissions: Vec<String>,
    /// Whether this plugin provides tools.
    pub provides_tools: bool,
    /// Whether this plugin provides commands.
    pub provides_commands: bool,
    /// Whether this plugin provides background jobs.
    pub provides_jobs: bool,
    /// Whether this plugin is enabled by default.
    pub enabled_by_default: bool,
}

impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            version: "0.1.0".to_string(),
            description: String::new(),
            author: String::new(),
            min_platform_version: "0.1.0".to_string(),
            permissions: Vec::new(),
            provides_tools: false,
            provides_commands: false,
            provides_jobs: false,
            enabled_by_default: true,
        }
    }
}
