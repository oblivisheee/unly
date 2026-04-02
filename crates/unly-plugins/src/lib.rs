//! Plugin system for the unly agent platform.
//!
//! Plugins extend the platform with:
//! - Custom slash commands
//! - Additional tools
//! - Background jobs
//! - Lifecycle hooks

pub mod error;
pub mod manifest;
pub mod plugin;
pub mod registry;

pub use error::PluginError;
pub use manifest::PluginManifest;
pub use plugin::Plugin;
pub use registry::PluginRegistry;
