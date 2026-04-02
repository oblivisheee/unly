//! Configuration loading, validation, and management for the unly agent platform.

pub mod config;
pub mod error;
pub mod loader;
pub mod secrets;

pub use config::*;
pub use error::ConfigError;
pub use loader::load_config;

/// Create a default AppConfig instance (useful for init-config and setup).
pub fn default_config() -> AppConfig {
    AppConfig::default()
}
