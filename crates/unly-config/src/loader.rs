use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use std::path::Path;
use tracing::info;

use crate::config::AppConfig;
use crate::error::ConfigError;
use crate::workspace;

/// Load configuration from a TOML file, with environment variable overrides.
///
/// Environment variables use the prefix `UNLY_` with `__` as the separator.
/// Example: `UNLY_TELEGRAM__BOT_TOKEN=...`
pub fn load_config(config_path: impl AsRef<Path>) -> Result<AppConfig, ConfigError> {
    let path = config_path.as_ref();

    info!("loading configuration from {}", path.display());

    let figment = if path.exists() {
        Figment::new()
            .merge(Toml::file(path))
            .merge(Env::prefixed("UNLY_").split("__"))
    } else {
        Figment::new().merge(Env::prefixed("UNLY_").split("__"))
    };

    let mut config: AppConfig = figment
        .extract()
        .map_err(|e| ConfigError::Parse(e.to_string()))?;

    // Override from well-known single-level env vars for convenience.
    apply_env_overrides(&mut config);

    validate_config(&config)?;

    Ok(config)
}

/// Load a default config (useful for first-run / onboarding).
pub fn default_config() -> AppConfig {
    AppConfig::default()
}

/// Return the path that should be used as the default config file.
///
/// Resolution order:
/// 1. `$UNLY_CONFIG` env var
/// 2. `~/.unly/config.toml` (workspace default)
pub fn resolve_default_config_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("UNLY_CONFIG") {
        return std::path::PathBuf::from(p);
    }
    workspace::default_config_path()
}

fn apply_env_overrides(config: &mut AppConfig) {
    if let Ok(token) = std::env::var("TELEGRAM_BOT_TOKEN")
        && !token.is_empty()
    {
        config.telegram.bot_token = token;
    }
    if let Ok(admins) = std::env::var("TELEGRAM_ADMIN_USER_IDS") {
        let ids: Vec<i64> = admins
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if !ids.is_empty() {
            config.telegram.admin_user_ids = ids;
        }
    }
}

fn validate_config(config: &AppConfig) -> Result<(), ConfigError> {
    if config.telegram.bot_token.is_empty() {
        return Err(ConfigError::MissingField(
            "telegram.bot_token (or TELEGRAM_BOT_TOKEN env var)".to_string(),
        ));
    }
    if config.telegram.admin_user_ids.is_empty() {
        return Err(ConfigError::Validation {
            field: "telegram.admin_user_ids".to_string(),
            message: "at least one admin user ID must be configured".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_serializable() {
        let config = default_config();
        let toml = toml::to_string(&config).expect("should serialize to TOML");
        assert!(toml.contains("bot_token"));
    }
}
