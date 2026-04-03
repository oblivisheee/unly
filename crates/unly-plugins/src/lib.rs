//! Plugin system for the unly agent platform.
//!
//! Plugins extend the platform with:
//! - Custom slash commands
//! - Additional tools
//! - Background jobs
//! - Lifecycle hooks
//!
//! Skills are lightweight, file-based agent capability extensions loaded from
//! directories that contain a `SKILL.md` file.

pub mod error;
pub mod manifest;
pub mod plugin;
pub mod plugin_loader;
pub mod registry;
pub mod skill;
pub mod skill_loader;

pub use error::PluginError;
pub use manifest::PluginManifest;
pub use plugin::Plugin;
pub use plugin_loader::{LoadedPlugin, PluginLoader, PluginMeta};
pub use registry::PluginRegistry;
pub use skill::{Skill, SkillMeta};
pub use skill_loader::SkillLoader;
