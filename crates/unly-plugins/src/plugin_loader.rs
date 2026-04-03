//! Filesystem-backed plugin loader.
//!
//! Scans a directory for plugin sub-directories (each must contain a
//! `PLUGIN.md` file), parses them, and provides helpers for installing,
//! removing, enabling, and disabling plugins on disk.
//!
//! # PLUGIN.md format
//! ```text
//! ---
//! name: my-plugin-name
//! description: A short description of what this plugin does
//! ---
//! # Instructions
//! ...markdown body...
//! ```
//!
//! Required frontmatter keys: `name`.
//! Optional frontmatter keys: `description`, `version`, `author`.

use std::path::{Path, PathBuf};

use tracing::{info, warn};

/// Frontmatter metadata parsed from `PLUGIN.md`.
#[derive(Debug, Clone, Default)]
pub struct PluginMeta {
    /// Unique plugin identifier derived from the frontmatter `name` field.
    pub name: String,
    /// Human-readable short description.
    pub description: String,
    /// Optional semver string.
    pub version: String,
    /// Optional author.
    pub author: String,
}

/// A loaded file-based plugin.
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    /// Parsed metadata.
    pub meta: PluginMeta,
    /// The Markdown instruction body (everything after the frontmatter).
    pub instructions: String,
    /// Path of the plugin directory on disk.
    pub path: PathBuf,
    /// Whether this plugin is currently active (no `.disabled` marker present).
    pub enabled: bool,
}

impl LoadedPlugin {
    /// Parse the content of a `PLUGIN.md` file.
    ///
    /// Returns `None` if the file does not contain valid frontmatter with a
    /// `name` key.
    pub fn from_plugin_md(content: &str, path: PathBuf, enabled: bool) -> Option<Self> {
        let meta = parse_frontmatter(content)?;
        let instructions = strip_frontmatter(content).trim().to_string();
        Some(Self {
            meta,
            instructions,
            path,
            enabled,
        })
    }
}

/// Marker file placed inside a plugin directory to mark it as disabled.
const DISABLED_MARKER: &str = ".disabled";

/// Handles loading and management of filesystem-backed plugins.
pub struct PluginLoader;

impl PluginLoader {
    /// Load all plugins found in `dir`.
    ///
    /// Sub-directories without a `PLUGIN.md` file are silently skipped.
    /// Parse errors are logged as warnings and also skipped.
    pub fn load_from_dir(dir: &Path) -> Vec<LoadedPlugin> {
        let mut plugins = Vec::new();

        if !dir.exists() {
            return plugins;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(err) => {
                warn!("failed to read plugins directory {}: {}", dir.display(), err);
                return plugins;
            }
        };

        for entry in entries.flatten() {
            let plugin_dir = entry.path();
            if !plugin_dir.is_dir() {
                continue;
            }

            let plugin_md = plugin_dir.join("PLUGIN.md");
            if !plugin_md.exists() {
                continue;
            }

            let content = match std::fs::read_to_string(&plugin_md) {
                Ok(c) => c,
                Err(err) => {
                    warn!("failed to read {}: {}", plugin_md.display(), err);
                    continue;
                }
            };

            let enabled = !plugin_dir.join(DISABLED_MARKER).exists();

            match LoadedPlugin::from_plugin_md(&content, plugin_dir.clone(), enabled) {
                Some(plugin) => {
                    info!(
                        "loaded plugin '{}' from {} (enabled={})",
                        plugin.meta.name,
                        plugin_dir.display(),
                        enabled
                    );
                    plugins.push(plugin);
                }
                None => {
                    warn!(
                        "skipping {}: PLUGIN.md missing required 'name' frontmatter field",
                        plugin_dir.display()
                    );
                }
            }
        }

        // Sort for deterministic ordering.
        plugins.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
        plugins
    }

    /// Install a plugin from a source directory into `plugins_dir`.
    ///
    /// # Errors
    /// Returns an error string if:
    /// - `src` has no `PLUGIN.md` (or it cannot be parsed).
    /// - A plugin with the same directory name already exists in `plugins_dir`.
    /// - A filesystem operation fails.
    pub fn install(src: &Path, plugins_dir: &Path) -> Result<String, String> {
        let plugin_md_path = src.join("PLUGIN.md");

        if !plugin_md_path.exists() {
            return Err(format!(
                "no PLUGIN.md found in '{}' — not a valid plugin directory",
                src.display()
            ));
        }

        let content = std::fs::read_to_string(&plugin_md_path)
            .map_err(|e| format!("failed to read PLUGIN.md: {}", e))?;

        let plugin =
            LoadedPlugin::from_plugin_md(&content, src.to_path_buf(), true).ok_or_else(|| {
                "failed to parse PLUGIN.md: 'name' field is required in frontmatter".to_string()
            })?;

        let dir_name = src
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(plugin.meta.name.as_str());
        let dest = plugins_dir.join(dir_name);

        if dest.exists() {
            return Err(format!(
                "plugin '{}' already exists at '{}'.  Remove it first with `unly plugin remove {}`",
                dir_name,
                dest.display(),
                dir_name
            ));
        }

        std::fs::create_dir_all(plugins_dir)
            .map_err(|e| format!("failed to create plugins directory: {}", e))?;

        copy_dir(src, &dest).map_err(|e| format!("failed to copy plugin directory: {}", e))?;

        info!(
            "installed plugin '{}' from '{}'",
            plugin.meta.name,
            src.display()
        );
        Ok(plugin.meta.name)
    }

    /// Remove the plugin whose directory name matches `id` from `plugins_dir`.
    pub fn remove(id: &str, plugins_dir: &Path) -> Result<(), String> {
        let target = plugins_dir.join(id);
        if !target.exists() {
            return Err(format!(
                "plugin '{}' not found in '{}'",
                id,
                plugins_dir.display()
            ));
        }
        std::fs::remove_dir_all(&target)
            .map_err(|e| format!("failed to remove plugin '{}': {}", id, e))?;
        info!("removed plugin '{}'", id);
        Ok(())
    }

    /// Mark the plugin directory `id` as disabled by creating a `.disabled`
    /// marker file inside it.
    pub fn disable(id: &str, plugins_dir: &Path) -> Result<(), String> {
        let target = plugins_dir.join(id);
        if !target.exists() {
            return Err(format!("plugin '{}' not found", id));
        }
        let marker = target.join(DISABLED_MARKER);
        if marker.exists() {
            return Ok(()); // already disabled
        }
        std::fs::write(&marker, b"")
            .map_err(|e| format!("failed to disable plugin '{}': {}", id, e))?;
        info!("disabled plugin '{}'", id);
        Ok(())
    }

    /// Re-enable a previously disabled plugin by removing its `.disabled`
    /// marker file.
    pub fn enable(id: &str, plugins_dir: &Path) -> Result<(), String> {
        let target = plugins_dir.join(id);
        if !target.exists() {
            return Err(format!("plugin '{}' not found", id));
        }
        let marker = target.join(DISABLED_MARKER);
        if !marker.exists() {
            return Ok(()); // already enabled
        }
        std::fs::remove_file(&marker)
            .map_err(|e| format!("failed to enable plugin '{}': {}", id, e))?;
        info!("enabled plugin '{}'", id);
        Ok(())
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Recursively copy a directory tree from `src` to `dst`.
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

/// Parse the YAML-style frontmatter between `---` fences.
fn parse_frontmatter(content: &str) -> Option<PluginMeta> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return None;
    }
    let rest = content.get(3..)?;
    let close = rest.find("\n---")?;
    let frontmatter = &rest[..close];

    let mut name: Option<String> = None;
    let mut description = String::new();
    let mut version = String::new();
    let mut author = String::new();

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            name = Some(clean_yaml_value(val));
        } else if let Some(val) = line.strip_prefix("description:") {
            description = clean_yaml_value(val);
        } else if let Some(val) = line.strip_prefix("version:") {
            version = clean_yaml_value(val);
        } else if let Some(val) = line.strip_prefix("author:") {
            author = clean_yaml_value(val);
        }
    }

    Some(PluginMeta {
        name: name?,
        description,
        version,
        author,
    })
}

/// Strip leading/trailing whitespace and optional surrounding quotes.
fn clean_yaml_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Return the Markdown body that follows the closing `---` fence.
fn strip_frontmatter(content: &str) -> &str {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return content;
    }
    let rest = match content.get(3..) {
        Some(r) => r,
        None => return content,
    };
    if let Some(end) = rest.find("\n---") {
        let after_close = &rest[end + 4..];
        after_close.trim_start_matches('\n')
    } else {
        content
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("unly-plugin-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_plugin(dir: &Path, name: &str) {
        let content = format!("---\nname: {}\ndescription: test\n---\n# Body\n", name);
        fs::write(dir.join("PLUGIN.md"), content).unwrap();
    }

    #[test]
    fn load_from_empty_dir() {
        let dir = tmp_dir();
        let plugins = PluginLoader::load_from_dir(&dir);
        assert!(plugins.is_empty());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn load_from_dir_finds_plugins() {
        let plugins_dir = tmp_dir();
        let plugin_a = plugins_dir.join("plugin-a");
        let plugin_b = plugins_dir.join("plugin-b");
        let not_plugin = plugins_dir.join("no-plugin-md");
        fs::create_dir_all(&plugin_a).unwrap();
        fs::create_dir_all(&plugin_b).unwrap();
        fs::create_dir_all(&not_plugin).unwrap();
        write_plugin(&plugin_a, "plugin-a");
        write_plugin(&plugin_b, "plugin-b");

        let plugins = PluginLoader::load_from_dir(&plugins_dir);
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].meta.name, "plugin-a");
        assert_eq!(plugins[1].meta.name, "plugin-b");

        fs::remove_dir_all(plugins_dir).ok();
    }

    #[test]
    fn disabled_marker_toggles_plugin_state() {
        let plugins_dir = tmp_dir();
        let plugin_dir = plugins_dir.join("my-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        write_plugin(&plugin_dir, "my-plugin");

        let plugins = PluginLoader::load_from_dir(&plugins_dir);
        assert!(plugins[0].enabled);

        PluginLoader::disable("my-plugin", &plugins_dir).unwrap();
        let plugins = PluginLoader::load_from_dir(&plugins_dir);
        assert!(!plugins[0].enabled);

        PluginLoader::enable("my-plugin", &plugins_dir).unwrap();
        let plugins = PluginLoader::load_from_dir(&plugins_dir);
        assert!(plugins[0].enabled);

        fs::remove_dir_all(plugins_dir).ok();
    }

    #[test]
    fn install_and_remove() {
        let src = tmp_dir();
        let plugins_dir = tmp_dir();
        write_plugin(&src, "install-test");

        let name = PluginLoader::install(&src, &plugins_dir).unwrap();
        assert!(!name.is_empty());

        let dest_name = src.file_name().unwrap().to_str().unwrap();
        assert!(plugins_dir.join(dest_name).exists());

        PluginLoader::remove(dest_name, &plugins_dir).unwrap();
        assert!(!plugins_dir.join(dest_name).exists());

        fs::remove_dir_all(src).ok();
        fs::remove_dir_all(plugins_dir).ok();
    }

    #[test]
    fn parses_valid_plugin_md() {
        let md = "---\nname: test-plugin\ndescription: A test\nversion: 1.0.0\nauthor: Tester\n---\n# Body\n\nDo stuff.\n";
        let plugin = LoadedPlugin::from_plugin_md(md, PathBuf::from("/tmp"), true).unwrap();
        assert_eq!(plugin.meta.name, "test-plugin");
        assert_eq!(plugin.meta.description, "A test");
        assert_eq!(plugin.meta.version, "1.0.0");
        assert_eq!(plugin.meta.author, "Tester");
        assert!(plugin.instructions.contains("Do stuff"));
    }
}
