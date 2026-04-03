use std::path::Path;
use tracing::warn;
use zeroize::Zeroizing;

/// A secret value that is zeroed on drop.
pub type Secret = Zeroizing<String>;

/// Load a secret from an environment variable, returning an error if not set.
pub fn require_secret(env_var: &str) -> Result<Secret, String> {
    std::env::var(env_var)
        .map(Zeroizing::new)
        .map_err(|_| format!("required secret env var not set: {}", env_var))
}

/// Load a secret from a file, returning an error if the file does not exist.
pub fn load_secret_file(path: impl AsRef<Path>) -> std::io::Result<Secret> {
    let content = std::fs::read_to_string(path)?;
    Ok(Zeroizing::new(content.trim().to_string()))
}

/// Redact a secret value for safe logging/display.
pub fn redact(value: &str) -> String {
    if value.is_empty() {
        return "<empty>".to_string();
    }
    if value.len() <= 8 {
        return "*".repeat(value.len());
    }
    let visible_chars = 4;
    format!("{}...{}", &value[..visible_chars], "*".repeat(8))
}

/// Warn if a secret appears to be a placeholder or default value.
pub fn warn_if_placeholder(name: &str, value: &str) {
    let placeholders = [
        "your_token_here",
        "changeme",
        "placeholder",
        "TODO",
        "FIXME",
        "xxxx",
    ];
    for p in &placeholders {
        if value.contains(p) {
            warn!(
                "{} appears to contain a placeholder value — update before production use",
                name
            );
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_short_value() {
        assert_eq!(redact("abc"), "***");
    }

    #[test]
    fn redact_long_value() {
        let redacted = redact("super_secret_token_value");
        assert!(redacted.contains("..."));
        assert!(!redacted.contains("secret"));
    }

    #[test]
    fn redact_empty() {
        assert_eq!(redact(""), "<empty>");
    }
}
