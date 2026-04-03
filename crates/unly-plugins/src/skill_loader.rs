//! Filesystem-backed skill loader.
//!
//! Scans a directory for skill sub-directories (each must contain a
//! `SKILL.md` file), parses them, and provides helpers for installing,
//! removing, enabling, and disabling skills on disk.

use std::path::Path;

use tracing::{info, warn};

use crate::skill::Skill;

/// Marker file placed inside a skill directory to mark it as disabled.
const DISABLED_MARKER: &str = ".disabled";

/// Handles loading and management of filesystem-backed skills.
pub struct SkillLoader;

impl SkillLoader {
    /// Load all skills found in `dir`.
    ///
    /// Sub-directories without a `SKILL.md` file are silently skipped.
    /// Parse errors are logged as warnings and also skipped.
    pub fn load_from_dir(dir: &Path) -> Vec<Skill> {
        let mut skills = Vec::new();

        if !dir.exists() {
            return skills;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(err) => {
                warn!("failed to read skills directory {}: {}", dir.display(), err);
                return skills;
            }
        };

        for entry in entries.flatten() {
            let skill_dir = entry.path();
            if !skill_dir.is_dir() {
                continue;
            }

            let skill_md = skill_dir.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }

            let content = match std::fs::read_to_string(&skill_md) {
                Ok(c) => c,
                Err(err) => {
                    warn!("failed to read {}: {}", skill_md.display(), err);
                    continue;
                }
            };

            let enabled = !skill_dir.join(DISABLED_MARKER).exists();

            match Skill::from_skill_md(&content, skill_dir.clone(), enabled) {
                Some(skill) => {
                    info!(
                        "loaded skill '{}' from {} (enabled={})",
                        skill.meta.name,
                        skill_dir.display(),
                        enabled
                    );
                    skills.push(skill);
                }
                None => {
                    warn!(
                        "skipping {}: SKILL.md missing required 'name' frontmatter field",
                        skill_dir.display()
                    );
                }
            }
        }

        // Sort for deterministic ordering.
        skills.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
        skills
    }

    /// Install a skill from a source directory into `skills_dir`.
    ///
    /// # Errors
    /// Returns an error string if:
    /// - `src` has no `SKILL.md` (or it cannot be parsed).
    /// - A skill with the same directory name already exists in `skills_dir`.
    /// - A filesystem operation fails.
    pub fn install(src: &Path, skills_dir: &Path) -> Result<String, String> {
        let skill_md_path = src.join("SKILL.md");

        if !skill_md_path.exists() {
            return Err(format!(
                "no SKILL.md found in '{}' — not a valid skill directory",
                src.display()
            ));
        }

        let content = std::fs::read_to_string(&skill_md_path)
            .map_err(|e| format!("failed to read SKILL.md: {}", e))?;

        let skill = Skill::from_skill_md(&content, src.to_path_buf(), true).ok_or_else(|| {
            "failed to parse SKILL.md: 'name' field is required in frontmatter".to_string()
        })?;

        // Use the source directory's name as the install name.
        let dir_name = src
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(skill.meta.name.as_str());
        let dest = skills_dir.join(dir_name);

        if dest.exists() {
            return Err(format!(
                "skill '{}' already exists at '{}'.  Remove it first with `unly plugin remove {}`",
                dir_name,
                dest.display(),
                dir_name
            ));
        }

        std::fs::create_dir_all(skills_dir)
            .map_err(|e| format!("failed to create skills directory: {}", e))?;

        copy_dir(src, &dest).map_err(|e| format!("failed to copy skill directory: {}", e))?;

        Ok(skill.meta.name)
    }

    /// Remove the skill whose directory name matches `id` from `skills_dir`.
    ///
    /// # Errors
    /// Returns an error string if the skill directory is not found or the
    /// removal fails.
    pub fn remove(id: &str, skills_dir: &Path) -> Result<(), String> {
        let target = skills_dir.join(id);
        if !target.exists() {
            return Err(format!("skill '{}' not found in '{}'", id, skills_dir.display()));
        }
        std::fs::remove_dir_all(&target)
            .map_err(|e| format!("failed to remove skill '{}': {}", id, e))
    }

    /// Mark the skill directory `id` as disabled by creating a `.disabled`
    /// marker file inside it.
    pub fn disable(id: &str, skills_dir: &Path) -> Result<(), String> {
        let target = skills_dir.join(id);
        if !target.exists() {
            return Err(format!("skill '{}' not found", id));
        }
        let marker = target.join(DISABLED_MARKER);
        if marker.exists() {
            return Ok(()); // already disabled
        }
        std::fs::write(&marker, b"")
            .map_err(|e| format!("failed to disable skill '{}': {}", id, e))
    }

    /// Re-enable a previously disabled skill by removing its `.disabled`
    /// marker file.
    pub fn enable(id: &str, skills_dir: &Path) -> Result<(), String> {
        let target = skills_dir.join(id);
        if !target.exists() {
            return Err(format!("skill '{}' not found", id));
        }
        let marker = target.join(DISABLED_MARKER);
        if !marker.exists() {
            return Ok(()); // already enabled
        }
        std::fs::remove_file(&marker)
            .map_err(|e| format!("failed to enable skill '{}': {}", id, e))
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("unly-skill-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_skill(dir: &Path, name: &str) {
        let content = format!("---\nname: {}\ndescription: test\n---\n# Body\n", name);
        fs::write(dir.join("SKILL.md"), content).unwrap();
    }

    #[test]
    fn load_from_empty_dir() {
        let dir = tmp_dir();
        let skills = SkillLoader::load_from_dir(&dir);
        assert!(skills.is_empty());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn load_from_dir_finds_skills() {
        let skills_dir = tmp_dir();
        let skill_a = skills_dir.join("skill-a");
        let skill_b = skills_dir.join("skill-b");
        let not_skill = skills_dir.join("no-skill-md");
        fs::create_dir_all(&skill_a).unwrap();
        fs::create_dir_all(&skill_b).unwrap();
        fs::create_dir_all(&not_skill).unwrap();
        write_skill(&skill_a, "skill-a");
        write_skill(&skill_b, "skill-b");

        let skills = SkillLoader::load_from_dir(&skills_dir);
        assert_eq!(skills.len(), 2);
        // sorted by name
        assert_eq!(skills[0].meta.name, "skill-a");
        assert_eq!(skills[1].meta.name, "skill-b");

        fs::remove_dir_all(skills_dir).ok();
    }

    #[test]
    fn disabled_marker_toggles_skill_state() {
        let skills_dir = tmp_dir();
        let skill_dir = skills_dir.join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        write_skill(&skill_dir, "my-skill");

        // Initially enabled.
        let skills = SkillLoader::load_from_dir(&skills_dir);
        assert!(skills[0].enabled);

        // Disable.
        SkillLoader::disable("my-skill", &skills_dir).unwrap();
        let skills = SkillLoader::load_from_dir(&skills_dir);
        assert!(!skills[0].enabled);

        // Re-enable.
        SkillLoader::enable("my-skill", &skills_dir).unwrap();
        let skills = SkillLoader::load_from_dir(&skills_dir);
        assert!(skills[0].enabled);

        fs::remove_dir_all(skills_dir).ok();
    }

    #[test]
    fn install_and_remove() {
        let src = tmp_dir();
        let skills_dir = tmp_dir();
        write_skill(&src, "install-test");

        let name = SkillLoader::install(&src, &skills_dir).unwrap();
        assert!(!name.is_empty());

        let dest_name = src.file_name().unwrap().to_str().unwrap();
        assert!(skills_dir.join(dest_name).exists());

        SkillLoader::remove(dest_name, &skills_dir).unwrap();
        assert!(!skills_dir.join(dest_name).exists());

        fs::remove_dir_all(src).ok();
        fs::remove_dir_all(skills_dir).ok();
    }

    #[test]
    fn install_fails_without_skill_md() {
        let src = tmp_dir();
        let skills_dir = tmp_dir();

        let result = SkillLoader::install(&src, &skills_dir);
        assert!(result.is_err());

        fs::remove_dir_all(src).ok();
        fs::remove_dir_all(skills_dir).ok();
    }

    #[test]
    fn install_fails_for_duplicate() {
        let src = tmp_dir();
        let skills_dir = tmp_dir();
        write_skill(&src, "dup-skill");

        SkillLoader::install(&src, &skills_dir).unwrap();
        let result = SkillLoader::install(&src, &skills_dir);
        assert!(result.is_err(), "second install should fail");

        fs::remove_dir_all(src).ok();
        fs::remove_dir_all(skills_dir).ok();
    }
}
