//! Skill support — lightweight, file-based agent capability extensions.
//!
//! A skill is a directory that contains at least a `SKILL.md` file with YAML
//! frontmatter followed by Markdown instructions.  Skills are loaded at agent
//! start-up and their instructions are injected into the system prompt so the
//! agent knows how to use each capability.
//!
//! # SKILL.md format
//! ```text
//! ---
//! name: my-skill-name
//! description: A short description of what this skill does
//! ---
//! # Instructions
//! ...markdown body...
//! ```
//!
//! Required frontmatter keys: `name`.
//! Optional frontmatter keys: `description`, `version`, `author`.

use std::path::PathBuf;

/// Frontmatter metadata parsed from `SKILL.md`.
#[derive(Debug, Clone, Default)]
pub struct SkillMeta {
    /// Unique skill identifier derived from the frontmatter `name` field.
    pub name: String,
    /// Human-readable short description.
    pub description: String,
    /// Optional semver string.
    pub version: String,
    /// Optional author.
    pub author: String,
}

/// A loaded skill.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Parsed metadata.
    pub meta: SkillMeta,
    /// The Markdown instruction body (everything after the frontmatter).
    pub instructions: String,
    /// Path of the skill directory on disk.
    pub path: PathBuf,
    /// Whether this skill is currently active (no `.disabled` marker present).
    pub enabled: bool,
}

impl Skill {
    /// Parse the content of a `SKILL.md` file.
    ///
    /// Returns `None` if the file does not contain valid frontmatter with a
    /// `name` key.
    pub fn from_skill_md(content: &str, path: PathBuf, enabled: bool) -> Option<Self> {
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

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Parse the YAML-style frontmatter between `---` fences.
///
/// Only simple `key: value` lines are supported — no nested structures.
fn parse_frontmatter(content: &str) -> Option<SkillMeta> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return None;
    }
    // Skip past the opening `---`
    let rest = content.get(3..)?;

    // Find the closing `---` fence.  It must appear at the start of a line.
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

    Some(SkillMeta {
        name: name?,
        description,
        version,
        author,
    })
}

/// Strip leading/trailing whitespace and optional surrounding quotes.
fn clean_yaml_value(raw: &str) -> String {
    let trimmed = raw.trim();
    // Remove matching single or double quotes if present.
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
    // Find "\n---" which marks the end of the frontmatter block.
    if let Some(end) = rest.find("\n---") {
        let after_close = &rest[end + 4..]; // skip past "\n---"
        after_close.trim_start_matches('\n')
    } else {
        content
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const VALID_SKILL_MD: &str = r#"---
name: example-skill
description: A test skill
version: 0.1.0
author: Test Author
---
# Example Skill

Do something useful.
"#;

    const NO_FRONTMATTER: &str = "# Just a plain markdown file\n\nNo frontmatter here.";

    const MISSING_NAME: &str = r#"---
description: Missing name field
---
body
"#;

    #[test]
    fn parses_valid_skill_md() {
        let skill = Skill::from_skill_md(VALID_SKILL_MD, PathBuf::from("/tmp/skill"), true)
            .expect("should parse");
        assert_eq!(skill.meta.name, "example-skill");
        assert_eq!(skill.meta.description, "A test skill");
        assert_eq!(skill.meta.version, "0.1.0");
        assert_eq!(skill.meta.author, "Test Author");
        assert!(skill.instructions.contains("Do something useful"));
        assert!(skill.enabled);
    }

    #[test]
    fn returns_none_for_missing_frontmatter() {
        assert!(
            Skill::from_skill_md(NO_FRONTMATTER, PathBuf::from("/tmp"), true).is_none(),
            "plain markdown without frontmatter should return None"
        );
    }

    #[test]
    fn returns_none_when_name_missing() {
        assert!(
            Skill::from_skill_md(MISSING_NAME, PathBuf::from("/tmp"), true).is_none(),
            "frontmatter without name should return None"
        );
    }

    #[test]
    fn quoted_values_are_unquoted() {
        let md = "---\nname: \"quoted-name\"\ndescription: 'quoted desc'\n---\nbody";
        let skill =
            Skill::from_skill_md(md, PathBuf::from("/tmp"), true).expect("should parse quoted");
        assert_eq!(skill.meta.name, "quoted-name");
        assert_eq!(skill.meta.description, "quoted desc");
    }
}
