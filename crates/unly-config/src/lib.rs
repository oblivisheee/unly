//! Configuration loading, validation, and management for the unly agent platform.

pub mod config;
pub mod error;
pub mod loader;
pub mod secrets;
pub mod workspace;

pub use config::*;
pub use error::ConfigError;
pub use loader::load_config;
pub use workspace::{
    boot_path, default_config_path, default_db_path, default_token_cache_path, ensure_workspace,
    identity_path, workspace_dir, DEFAULT_BOOT, DEFAULT_IDENTITY,
};

/// Create a default AppConfig instance (useful for init-config and setup).
pub fn default_config() -> AppConfig {
    AppConfig::default()
}
