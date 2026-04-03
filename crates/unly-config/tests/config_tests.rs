//! Tests for configuration loading and defaults.

use unly_config::default_config;

#[test]
fn default_config_is_valid_structure() {
    let config = default_config();
    // Providers default to copilot
    assert_eq!(config.providers.default_provider, "copilot");
    assert!(!config.providers.default_model.is_empty());
    // Security is on by default
    assert!(config.security.redact_secrets);
    // Shell is restricted by default via allowlist
    assert!(!config.tools.shell_allowlist.is_empty());
    // Approval required for privileged tools by default
    assert!(config.tools.require_approval_for_privileged);
    assert!(config.tools.require_approval_for_dangerous);
    // Open access is off by default
    assert!(!config.telegram.open_access);
}

#[test]
fn default_config_has_safe_tool_defaults() {
    let config = default_config();
    let enabled = &config.tools.enabled_tools;
    assert!(enabled.contains(&"http_get".to_string()));
    assert!(enabled.contains(&"fs_read".to_string()));
    assert!(enabled.contains(&"git_status".to_string()));
    assert!(enabled.contains(&"bash".to_string()));
}

#[test]
fn default_database_path_is_relative() {
    let config = default_config();
    // Sanity check: the default database path should be under a user-owned
    // workspace (e.g. ~/.unly/data/unly.sqlite) and always end with the
    // expected filename, not be a bare system path like /var/db/... .
    let path = config.database.path.to_string_lossy();
    assert!(
        path.ends_with("unly.sqlite"),
        "expected database path to end with 'unly.sqlite', got: {}",
        path
    );
    // Must not be a bare system directory path (no plain /var, /etc, /usr prefix).
    let forbidden = ["/var/", "/etc/", "/usr/", "/sys/", "/proc/"];
    for prefix in &forbidden {
        assert!(
            !path.starts_with(prefix),
            "database path must not point to a system directory, got: {}",
            path
        );
    }
}

#[test]
fn default_config_serializes_to_toml() {
    let config = default_config();
    let toml = toml::to_string_pretty(&config);
    assert!(
        toml.is_ok(),
        "default config must serialize to TOML: {:?}",
        toml.err()
    );
    let content = toml.unwrap();
    assert!(content.contains("[telegram]"));
    assert!(content.contains("[database]"));
    assert!(content.contains("[providers]"));
}
