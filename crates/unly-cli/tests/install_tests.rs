//! End-to-end integration tests that exercise the installed `unly` binary.
//!
//! Each test spawns the real compiled binary via `CARGO_BIN_EXE_unly` (set
//! automatically by cargo when building integration tests) and inspects its
//! exit code and stdout/stderr output, closely mirroring what an operator
//! does when deploying the service for the first time.
//!
//! Tests are isolated: they use unique temporary directories and carry no
//! shared mutable state between runs.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// ── helpers ──────────────────────────────────────────────────────────────────

/// Absolute path to the compiled `unly` binary under test.
fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_unly"))
}

/// Create an isolated temp workspace directory under `/tmp/unly-tests/<name>`.
/// Returns the path; the caller owns the directory for the test lifetime.
fn tmp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir()
            .join("unly-tests")
            .join(format!("{}-{}", name, std::process::id()));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Run the binary with optional args, env vars, and a forced `UNLY_HOME`.
fn run(args: &[&str], unly_home: &Path, extra_env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(binary());
    cmd.args(args)
        .env("UNLY_HOME", unly_home)
        // Suppress interactive prompts in CI.
        .env("TERM", "dumb")
        // Silence tracing noise in test output.
        .env("RUST_LOG", "error");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("spawn unly binary")
}

/// Assert the command succeeded (exit 0) and return stdout as a String.
#[track_caller]
fn assert_success(out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "expected exit 0 but got {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        stdout,
        stderr,
    );
    stdout
}

/// Assert the command failed (non-zero exit) and return stderr as a String.
#[track_caller]
fn assert_failure(out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        !out.status.success(),
        "expected non-zero exit but got 0\nstdout: {}\nstderr: {}",
        stdout,
        stderr,
    );
    // Concatenate both streams — the binary sometimes writes errors to stdout.
    format!("{}\n{}", stdout, stderr)
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// `unly --version` must exit 0 and print the package version string.
#[test]
fn version_flag_exits_zero_and_prints_version() {
    let home = tmp_dir("version");
    let out = run(&["--version"], &home, &[]);
    let stdout = assert_success(&out);
    // Cargo embeds the version from Cargo.toml at compile time.
    let expected = env!("CARGO_PKG_VERSION");
    assert!(
        stdout.contains(expected),
        "expected version string '{}' in stdout: {}",
        expected,
        stdout
    );
}

/// `unly init-config <path>` must exit 0 and write a parseable TOML file that
/// contains the mandatory `[telegram]`, `[database]`, and `[providers]` sections.
#[test]
fn init_config_creates_valid_toml_file() {
    let home = tmp_dir("init-config");
    let config_path = home.join("config.toml");

    let out = run(&["init-config", config_path.to_str().unwrap()], &home, &[]);
    assert_success(&out);

    assert!(
        config_path.exists(),
        "init-config must create the file at the given path"
    );

    let content = fs::read_to_string(&config_path).expect("read config file");

    // Must be valid TOML.
    let parsed: toml::Value = toml::from_str(&content).expect("init-config must write valid TOML");

    // Must contain the top-level sections required for the service to start.
    for section in &["telegram", "database", "providers", "tools", "logging"] {
        assert!(
            parsed.get(section).is_some(),
            "generated config must contain [{}] section",
            section
        );
    }

    // Database path must end with unly.sqlite — never a bare system path.
    let db_path = parsed["database"]["path"].as_str().unwrap_or("");
    assert!(
        db_path.ends_with("unly.sqlite"),
        "database.path must end with 'unly.sqlite', got: {}",
        db_path
    );
}

/// Running `init-config` twice on the same path must fail with a clear error
/// and must NOT silently overwrite the existing file.
#[test]
fn init_config_refuses_to_overwrite_existing_file() {
    let home = tmp_dir("init-config-overwrite");
    let config_path = home.join("config.toml");

    // First call — should succeed.
    let out1 = run(&["init-config", config_path.to_str().unwrap()], &home, &[]);
    assert_success(&out1);
    let original = fs::read_to_string(&config_path).expect("read config file");

    // Second call on the same path — must fail.
    let out2 = run(&["init-config", config_path.to_str().unwrap()], &home, &[]);
    let error_msg = assert_failure(&out2);
    assert!(
        error_msg.to_lowercase().contains("already exists")
            || error_msg.to_lowercase().contains("exist"),
        "error message must mention that the file already exists: {}",
        error_msg
    );

    // File contents must be unchanged.
    let after = fs::read_to_string(&config_path).expect("read config file");
    assert_eq!(
        original, after,
        "existing config file must not be modified on a second init-config call"
    );
}

/// `unly validate` with valid credentials in env vars must exit 0 and print
/// "valid" (case-insensitive) somewhere in its output.
#[test]
fn validate_succeeds_with_env_credentials() {
    let home = tmp_dir("validate-ok");

    // First create a minimal config file (bot_token + admin filled via env).
    let config_path = home.join("config.toml");
    run(&["init-config", config_path.to_str().unwrap()], &home, &[]);

    let out = run(
        &["--config", config_path.to_str().unwrap(), "validate"],
        &home,
        &[
            (
                "TELEGRAM_BOT_TOKEN",
                "1234567890:AAFakeTokenForTestingPurposesOnly",
            ),
            ("TELEGRAM_ADMIN_USER_IDS", "123456789"),
        ],
    );
    let stdout = assert_success(&out);
    assert!(
        stdout.to_lowercase().contains("valid"),
        "validate output must mention 'valid': {}",
        stdout
    );
}

/// `unly validate` without a bot token must exit non-zero and mention the
/// missing field in the error output.
#[test]
fn validate_fails_without_bot_token() {
    let home = tmp_dir("validate-fail");

    let config_path = home.join("config.toml");
    run(&["init-config", config_path.to_str().unwrap()], &home, &[]);

    let out = run(
        &["--config", config_path.to_str().unwrap(), "validate"],
        &home,
        // Deliberately omit TELEGRAM_BOT_TOKEN.
        &[("TELEGRAM_ADMIN_USER_IDS", "123456789")],
    );
    let err = assert_failure(&out);
    assert!(
        err.to_lowercase().contains("bot_token") || err.to_lowercase().contains("token"),
        "error must mention missing bot_token: {}",
        err
    );
}

/// `unly validate` without admin IDs must exit non-zero and mention the
/// missing field in the error output.
#[test]
fn validate_fails_without_admin_ids() {
    let home = tmp_dir("validate-no-admins");

    let config_path = home.join("config.toml");
    run(&["init-config", config_path.to_str().unwrap()], &home, &[]);

    let out = run(
        &["--config", config_path.to_str().unwrap(), "validate"],
        &home,
        // Provide token but NO admin IDs.
        &[(
            "TELEGRAM_BOT_TOKEN",
            "1234567890:AAFakeTokenForTestingPurposesOnly",
        )],
    );
    let err = assert_failure(&out);
    assert!(
        err.to_lowercase().contains("admin") || err.to_lowercase().contains("user_id"),
        "error must mention missing admin_user_ids: {}",
        err
    );
}

/// `unly migrate` against a brand-new SQLite database must exit 0 and print
/// something indicating completion.  This exercises the full db-init path
/// that happens on the very first service start.
#[test]
fn migrate_runs_successfully_against_new_database() {
    let home = tmp_dir("migrate");
    let config_path = home.join("config.toml");

    // Generate config file pointing at a fresh db in our temp dir.
    run(&["init-config", config_path.to_str().unwrap()], &home, &[]);

    let out = run(
        &["--config", config_path.to_str().unwrap(), "migrate"],
        &home,
        &[
            (
                "TELEGRAM_BOT_TOKEN",
                "1234567890:AAFakeTokenForTestingPurposesOnly",
            ),
            ("TELEGRAM_ADMIN_USER_IDS", "123456789"),
        ],
    );
    let stdout = assert_success(&out);
    assert!(
        stdout.to_lowercase().contains("migrat"),
        "migrate output must mention migrations: {}",
        stdout
    );

    // The database file must now exist.
    let db_path = home.join("data").join("unly.sqlite");
    assert!(
        db_path.exists(),
        "SQLite database file must exist after migrate: {}",
        db_path.display()
    );
}

/// Running migrate twice must be idempotent — exit 0 both times.
#[test]
fn migrate_is_idempotent() {
    let home = tmp_dir("migrate-idempotent");
    let config_path = home.join("config.toml");

    run(&["init-config", config_path.to_str().unwrap()], &home, &[]);

    let env_vars = &[
        (
            "TELEGRAM_BOT_TOKEN",
            "1234567890:AAFakeTokenForTestingPurposesOnly",
        ),
        ("TELEGRAM_ADMIN_USER_IDS", "123456789"),
    ];

    let out1 = run(
        &["--config", config_path.to_str().unwrap(), "migrate"],
        &home,
        env_vars,
    );
    assert_success(&out1);

    let out2 = run(
        &["--config", config_path.to_str().unwrap(), "migrate"],
        &home,
        env_vars,
    );
    assert_success(&out2);
}

// ── systemd unit template tests ───────────────────────────────────────────────

/// The bundled `deploy/unly.service` template must not contain any sandbox or
/// security hardening directives — the bot needs full system access.
#[test]
fn bundled_service_template_has_no_security_sandbox() {
    let template = include_str!("../../../deploy/unly.service");

    // None of the systemd sandbox / security hardening knobs must be present.
    for directive in &[
        "ProtectHome=",
        "ProtectSystem=",
        "NoNewPrivileges=",
        "PrivateTmp=",
        "ReadWritePaths=",
        "CapabilityBoundingSet=",
        "AmbientCapabilities=",
    ] {
        assert!(
            !template.contains(directive),
            "template must not contain security directive: {}",
            directive
        );
    }

    // Service user/group are rendered dynamically at install time.
    assert!(
        template.contains("User=__UNLY_SERVICE_USER__"),
        "template must contain User placeholder"
    );
    assert!(
        template.contains("Group=__UNLY_SERVICE_GROUP__"),
        "template must contain Group placeholder"
    );

    // Restart policy must be present so the service recovers from transient
    // failures (e.g. network unavailability at startup).
    assert!(
        template.contains("Restart=on-failure"),
        "template must contain Restart=on-failure"
    );

    // The unit must declare journald integration so logs are reachable via
    // `journalctl -u unly`.
    assert!(
        template.contains("StandardOutput=journal"),
        "template must route stdout to journald"
    );
    assert!(
        template.contains("SyslogIdentifier=unly"),
        "template must set SyslogIdentifier=unly"
    );

    // The unit must be parseable as text (no null bytes, valid UTF-8).
    assert!(!template.is_empty(), "template must not be empty");
}

/// The bundled `deploy/unly.service` must contain all mandatory systemd
/// section headers ([Unit], [Service], [Install]).
#[test]
fn bundled_service_template_has_required_sections() {
    let template = include_str!("../../../deploy/unly.service");

    for section in &["[Unit]", "[Service]", "[Install]"] {
        assert!(
            template.contains(section),
            "template must contain systemd section {}",
            section
        );
    }

    // [Install] must declare the standard multi-user target so
    // `systemctl enable unly` works correctly.
    assert!(
        template.contains("WantedBy=multi-user.target"),
        "template must declare WantedBy=multi-user.target"
    );
}

/// The bundled env example file must document the minimum required variables
/// that operators need to fill in before the service can start.
#[test]
fn env_example_documents_required_variables() {
    let example = include_str!("../../../deploy/unly.env.example");

    // These two are checked by validate_config() and will fail startup if absent.
    assert!(
        example.contains("TELEGRAM_BOT_TOKEN"),
        "env example must document TELEGRAM_BOT_TOKEN"
    );
    assert!(
        example.contains("TELEGRAM_ADMIN_USER_IDS"),
        "env example must document TELEGRAM_ADMIN_USER_IDS"
    );
}
