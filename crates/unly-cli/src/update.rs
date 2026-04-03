//! Self-update logic for the unly CLI.
//!
//! Fetches release metadata from GitHub, compares versions, and replaces the
//! running binary in-place when an update is available.

use anyhow::{Context, Result};
use serde::Deserialize;

const RELEASES_URL: &str = "https://api.github.com/repos/oblivisheee/unly/releases/latest";
const USER_AGENT: &str = concat!("unly/", env!("CARGO_PKG_VERSION"));

// ── GitHub API types ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Check whether a newer release is available on GitHub.
///
/// Returns `Some((current, latest, release_url))` when an update exists, or
/// `None` when the running version is already the latest.
pub async fn check_update() -> Result<Option<(String, String, String)>> {
    let release = fetch_latest_release().await?;

    let current = env!("CARGO_PKG_VERSION").to_string();
    let latest = release.tag_name.trim_start_matches('v').to_string();

    if is_newer(&current, &latest) {
        Ok(Some((current, latest, release.html_url)))
    } else {
        Ok(None)
    }
}

/// Download the latest release binary for the current platform and replace
/// the running executable.
///
/// If already up-to-date, prints a message and returns without doing anything.
pub async fn perform_update() -> Result<()> {
    let release = fetch_latest_release().await?;

    let current = env!("CARGO_PKG_VERSION");
    let latest = release.tag_name.trim_start_matches('v');

    if !is_newer(current, latest) {
        println!("Already up-to-date (v{}).", current);
        return Ok(());
    }

    println!("Current version : v{}", current);
    println!("Latest  version : v{}", latest);

    let suffix = platform_suffix();
    let asset = release
        .assets
        .iter()
        .find(|a| asset_matches(&a.name, &suffix))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no binary found for platform '{}' in release v{}.\n\
                 You can download manually from: {}",
                suffix,
                latest,
                release.html_url
            )
        })?;

    println!("Downloading {}...", asset.name);

    let client = build_client()?;
    let bytes = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .context("downloading release asset")?
        .error_for_status()
        .context("release asset download returned error status")?
        .bytes()
        .await
        .context("reading release asset body")?
        .to_vec();

    let current_exe =
        std::env::current_exe().context("could not determine path to current executable")?;

    // Write to a sibling temp file so the rename is atomic on the same
    // filesystem.
    let tmp = current_exe.with_extension("update.tmp");
    std::fs::write(&tmp, &bytes)
        .with_context(|| format!("writing temporary binary to {}", tmp.display()))?;

    #[cfg(unix)]
    set_executable(&tmp)?;

    std::fs::rename(&tmp, &current_exe)
        .with_context(|| format!("replacing {} with updated binary", current_exe.display()))?;

    println!(
        "Updated to v{}. Restart unly to apply the new version.",
        latest
    );
    Ok(())
}

// ── Internal helpers ──────────────────────────────────────────────────────────

async fn fetch_latest_release() -> Result<GithubRelease> {
    build_client()?
        .get(RELEASES_URL)
        .send()
        .await
        .context("fetching latest release metadata from GitHub")?
        .error_for_status()
        .context("GitHub API returned an error status")?
        .json::<GithubRelease>()
        .await
        .context("parsing release JSON from GitHub")
}

fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("building HTTP client")
}

/// Returns the platform-specific suffix used in release asset names.
///
/// Convention: `{arch}-{vendor}-{os}[-{abi}]` mirroring Rust target triples.
fn platform_suffix() -> String {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu".to_string(),
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu".to_string(),
        ("macos", "x86_64") => "x86_64-apple-darwin".to_string(),
        ("macos", "aarch64") => "aarch64-apple-darwin".to_string(),
        ("windows", "x86_64") => "x86_64-pc-windows-msvc".to_string(),
        (os, arch) => format!("{}-{}", arch, os),
    }
}

/// Returns true if the asset filename is likely the binary for this platform.
fn asset_matches(name: &str, suffix: &str) -> bool {
    // Accept exact match or a match with a .exe extension (Windows).
    name.contains(suffix)
}

/// Return true when `latest` is strictly greater than `current` (semver).
fn is_newer(current: &str, latest: &str) -> bool {
    parse_semver(latest)
        .zip(parse_semver(current))
        .map(|(l, c)| l > c)
        .unwrap_or(false)
}

fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.split('.').take(3);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

#[cfg(unix)]
fn set_executable(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path)
        .with_context(|| format!("reading metadata for {}", path.display()))?;
    let mut perms = meta.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("setting executable permission on {}", path.display()))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_returns_true_for_higher_version() {
        assert!(is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("1.0.0", "2.0.0"));
        assert!(is_newer("0.1.9", "0.2.0"));
        assert!(is_newer("0.1.0", "0.1.1"));
    }

    #[test]
    fn is_newer_returns_false_for_same_or_lower() {
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.1.0"));
        assert!(!is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn platform_suffix_is_non_empty() {
        assert!(!platform_suffix().is_empty());
    }

    #[test]
    fn asset_matches_suffix() {
        let suffix = "x86_64-unknown-linux-gnu";
        assert!(asset_matches("unly-x86_64-unknown-linux-gnu", suffix));
        assert!(asset_matches("unly-0.2.0-x86_64-unknown-linux-gnu", suffix));
        assert!(!asset_matches("unly-aarch64-apple-darwin", suffix));
    }
}
