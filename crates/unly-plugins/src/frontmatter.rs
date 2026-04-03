//! Shared frontmatter parsing utilities used by both `skill` and `plugin_loader`.
//!
//! Supports simple YAML-style `key: value` lines between `---` fences.

/// Parsed fields common to both skill and plugin frontmatter.
#[derive(Debug, Default)]
pub struct CommonMeta {
    pub name: Option<String>,
    pub description: String,
    pub version: String,
    pub author: String,
}

/// Parse the YAML-style frontmatter block between `---` fences.
///
/// Only simple `key: value` lines are supported — no nested structures.
/// Returns `None` if the content does not start with a `---` fence or has no
/// closing fence.
pub fn parse_common_frontmatter(content: &str) -> Option<CommonMeta> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return None;
    }
    let rest = content.get(3..)?;
    let close = rest.find("\n---")?;
    let frontmatter = &rest[..close];

    let mut meta = CommonMeta::default();

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            meta.name = Some(clean_yaml_value(val));
        } else if let Some(val) = line.strip_prefix("description:") {
            meta.description = clean_yaml_value(val);
        } else if let Some(val) = line.strip_prefix("version:") {
            meta.version = clean_yaml_value(val);
        } else if let Some(val) = line.strip_prefix("author:") {
            meta.author = clean_yaml_value(val);
        }
    }

    Some(meta)
}

/// Return the Markdown body that follows the closing `---` fence.
pub fn strip_frontmatter(content: &str) -> &str {
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

/// Strip leading/trailing whitespace and optional surrounding quotes from a
/// YAML scalar value.
pub fn clean_yaml_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Build the SKILL.md / PLUGIN.md frontmatter block from metadata fields.
/// Returns the `---\n...\n---\n` string ready to prepend to the instructions.
pub fn build_frontmatter(name: &str, description: &str, version: &str, author: &str) -> String {
    let mut fm = format!("---\nname: {}\n", name);
    if !description.is_empty() {
        fm.push_str(&format!("description: {}\n", description));
    }
    if !version.is_empty() {
        fm.push_str(&format!("version: {}\n", version));
    }
    if !author.is_empty() {
        fm.push_str(&format!("author: {}\n", author));
    }
    fm.push_str("---\n");
    fm
}
