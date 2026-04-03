use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

use unly_audit::AuditLogger;
use unly_config::{default_config, load_config, workspace};
use unly_db::Database;
use unly_plugins::{PluginLoader, SkillLoader};
use unly_providers::copilot::{CopilotProvider, DevicePollResult};
use unly_telegram::{SessionStore, TelegramBot};

use crate::{
    logging::{init_logging, init_logging_with_file},
    service::{build_providers, build_runtime, build_tools, build_tools_with_scheduler},
    update as self_update,
};

/// Unly - self-hosted personal AI agent platform.
#[derive(Parser)]
#[command(name = "unly", version, about = "Unly personal AI agent platform")]
pub struct Cli {
    /// Path to the configuration file.
    #[arg(short, long, env = "UNLY_CONFIG", global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

impl Cli {
    /// Resolve the config file path: CLI flag > UNLY_CONFIG env > workspace default.
    fn config_path(&self) -> PathBuf {
        self.config
            .clone()
            .unwrap_or_else(workspace::default_config_path)
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the Telegram bot and all subsystems.
    Start,

    /// Interactive first-run setup wizard.
    Setup,

    /// Validate configuration.
    Validate,

    /// Run diagnostics on all subsystems.
    Doctor,

    /// Authenticate with a provider.
    ProviderLogin {
        /// Provider name (e.g. "copilot").
        provider: String,
    },

    /// Show provider status.
    ProviderStatus,

    /// Manage plugins.
    #[command(subcommand)]
    Plugin(PluginCommands),

    /// Manage scheduled jobs.
    #[command(subcommand)]
    Job(JobCommands),

    /// Run pending database migrations.
    Migrate,

    /// Show recent audit log entries.
    Audit {
        /// Number of entries to show.
        #[arg(short, long, default_value = "20")]
        n: u64,
    },

    /// Memory management commands.
    #[command(subcommand)]
    Memory(MemoryCommands),

    /// Generate a default configuration file.
    InitConfig {
        /// Output file path (defaults to workspace config path).
        output: Option<PathBuf>,
    },

    /// Remove the entire Unly workspace (config, database, identity, cache).
    Uninstall {
        /// Skip all confirmation prompts.
        #[arg(long)]
        skip: bool,
    },

    /// Check for a newer release and optionally install it.
    Update {
        /// Only print whether an update is available without installing it.
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand)]
pub enum PluginCommands {
    /// List installed skills.
    List,
    /// Install a skill from a local directory.
    Install {
        /// Path to the skill directory (must contain a SKILL.md file).
        path: PathBuf,
    },
    /// Remove an installed skill by its directory name.
    Remove {
        /// Skill directory name (as shown by `unly plugin list`).
        id: String,
    },
    /// Enable a previously disabled skill.
    Enable {
        /// Skill directory name.
        id: String,
    },
    /// Disable a skill (keeps it installed but inactive).
    Disable {
        /// Skill directory name.
        id: String,
    },
}

#[derive(Subcommand)]
pub enum JobCommands {
    /// List all defined jobs.
    List,
    /// Run a job immediately.
    Run { id: String },
    /// Enable a job.
    Enable { id: String },
    /// Disable a job.
    Disable { id: String },
}

#[derive(Subcommand)]
pub enum MemoryCommands {
    /// List memory entries for a scope.
    List {
        /// Scope in format "type:id" (e.g. "chat:uuid").
        #[arg(short, long)]
        scope: String,
        /// Maximum entries to show.
        #[arg(short, long, default_value = "20")]
        n: i64,
    },
    /// Prune expired memory entries.
    Prune,
}

impl Cli {
    pub async fn run(self) -> Result<()> {
        let config_path = self.config_path();

        match self.command {
            Commands::Start => {
                let config = load_config(&config_path)
                    .with_context(|| format!("loading config from {}", config_path.display()))?;

                init_logging_with_file(
                    &config.logging.level,
                    config.logging.json,
                    config.logging.file.as_deref(),
                );
                info!(
                    "starting unly agent platform v{}",
                    env!("CARGO_PKG_VERSION")
                );

                // Connect to database using the full DatabaseConfig.
                let db = Database::connect_with_config(&config.database)
                    .await
                    .context("failed to connect to database")?;

                info!("database connected");

                // Build subsystems.
                let audit = Arc::new(AuditLogger::new(db.clone()));
                let providers = build_providers(&config).await?;
                let (tools, scheduler) = build_tools_with_scheduler(&config, db.clone());
                let runtime = build_runtime(
                    &config,
                    providers.clone(),
                    tools,
                    db.clone(),
                    Some(audit.clone()),
                );
                let sessions = SessionStore::new();
                if config.scheduler.enabled {
                    tokio::spawn(async move {
                        scheduler.run().await;
                    });
                }

                let config_arc = Arc::new(config);
                let bot = Arc::new(TelegramBot::new(
                    config_arc.clone(),
                    sessions,
                    runtime,
                    providers,
                    db.clone(),
                    audit.clone(),
                ));

                info!("all subsystems initialized - starting Telegram bot");
                audit.success("startup", "system", "start");

                bot.start().await;
                audit.success("shutdown", "system", "ctrlc");
                info!("shutdown signal received - stopping unly gracefully");
                audit.flush().await;
                info!("audit logger flushed");
                db.close().await;
                info!("database connection closed");

                Ok(())
            }

            Commands::Setup => run_setup_wizard(&config_path).await,

            Commands::Validate => {
                match load_config(&config_path) {
                    Ok(config) => {
                        println!("Configuration is valid");
                        println!("  Telegram bot token: configured");
                        println!("  Admin user IDs: {:?}", config.telegram.admin_user_ids);
                        println!("  Default provider: {}", config.providers.default_provider);
                        println!("  Default model: {}", config.providers.default_model);
                        println!("  Database type: {:?}", config.database.db_type);
                        println!("  Database path: {}", config.database.path.display());
                    }
                    Err(e) => {
                        eprintln!("Configuration invalid: {}", e);
                        std::process::exit(1);
                    }
                }
                Ok(())
            }

            Commands::Doctor => {
                init_logging("info", false);

                let config = match load_config(&config_path) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("[FAIL] Config: {}", e);
                        std::process::exit(1);
                    }
                };

                println!("Unly Diagnostics\n");

                // Database check.
                match Database::connect_with_config(&config.database).await {
                    Ok(db) => match db.health_check().await {
                        Ok(_) => println!("[OK]   Database: ok"),
                        Err(e) => println!("[FAIL] Database: {}", e),
                    },
                    Err(e) => println!("[FAIL] Database: {}", e),
                }

                // Provider checks.
                let providers = build_providers(&config).await?;
                let reports = providers.health_all().await;
                for r in &reports {
                    let status = match r.status {
                        unly_core::types::HealthStatus::Healthy => "[OK]  ",
                        unly_core::types::HealthStatus::Degraded => "[WARN]",
                        unly_core::types::HealthStatus::Unhealthy => "[FAIL]",
                        unly_core::types::HealthStatus::Unknown => "[?]   ",
                    };
                    println!(
                        "{}  Provider {}: {}",
                        status,
                        r.name,
                        r.message.as_deref().unwrap_or("ok")
                    );
                }

                // Tool check.
                let tools = build_tools(&config);
                let tool_count = tools.list_schemas().len();
                println!("[OK]   Tools: {} registered", tool_count);

                println!("\nDiagnostics complete");
                Ok(())
            }

            Commands::ProviderLogin { provider } => {
                init_logging("info", false);

                let config = load_config(&config_path).or_else(|_| {
                    // Allow login even without a full valid config.
                    Ok::<_, anyhow::Error>(default_config())
                })?;

                match provider.as_str() {
                    "copilot" => {
                        let cp = CopilotProvider::new(
                            config.providers.copilot.github_client_id.clone(),
                            config.providers.copilot.token_cache_path.clone(),
                            config.providers.copilot.copilot_api_url.clone(),
                        );

                        println!("Authenticating with GitHub Copilot...\n");

                        // Start device flow.
                        let state = cp
                            .start_device_flow()
                            .await
                            .map_err(|e| anyhow::anyhow!("device flow start failed: {}", e))?;

                        println!("Open this URL in your browser:");
                        println!("  {}", state.verification_uri);
                        println!("\nEnter this code:");
                        println!("  {}", state.user_code);
                        println!("\nWaiting for authorization...\n");

                        // Poll until authorized.
                        let poll_interval = Duration::from_secs(state.interval.max(5));
                        let timeout = Duration::from_secs(state.expires_in);
                        let start = std::time::Instant::now();

                        loop {
                            if start.elapsed() > timeout {
                                bail!("device flow timed out");
                            }
                            tokio::time::sleep(poll_interval).await;
                            match cp.poll_device_flow(&state).await {
                                Ok(unly_providers::copilot::DevicePollResult::Authorized) => {
                                    println!("\nAuthenticated with GitHub Copilot.");
                                    println!(
                                        "Token cached at: {}",
                                        config.providers.copilot.token_cache_path.display()
                                    );
                                    break;
                                }
                                Ok(unly_providers::copilot::DevicePollResult::Pending) => {
                                    print!(".");
                                    use std::io::Write;
                                    std::io::stdout().flush().ok();
                                }
                                Ok(unly_providers::copilot::DevicePollResult::SlowDown) => {
                                    tokio::time::sleep(Duration::from_secs(5)).await;
                                }
                                Ok(unly_providers::copilot::DevicePollResult::Denied) => {
                                    bail!("authorization was denied");
                                }
                                Ok(unly_providers::copilot::DevicePollResult::Expired) => {
                                    bail!("device code expired - please try again");
                                }
                                Ok(unly_providers::copilot::DevicePollResult::Error(e)) => {
                                    bail!("authorization error: {}", e);
                                }
                                Err(e) => {
                                    bail!("polling error: {}", e);
                                }
                            }
                        }
                    }
                    _ => {
                        bail!("unknown provider: {}. Supported: copilot", provider);
                    }
                }

                Ok(())
            }

            Commands::ProviderStatus => {
                init_logging("info", false);
                let config = load_config(&config_path).unwrap_or_else(|_| default_config());
                let providers = build_providers(&config).await?;
                println!("Provider Status:\n");
                for name in providers.provider_names() {
                    if let Some(p) = providers.get(&name) {
                        let report = p.health().await;
                        let status = match report.status {
                            unly_core::types::HealthStatus::Healthy => "[OK]  ",
                            unly_core::types::HealthStatus::Degraded => "[WARN]",
                            _ => "[FAIL]",
                        };
                        println!(
                            "  {} {} - {}",
                            status,
                            name,
                            report.message.as_deref().unwrap_or("ok")
                        );
                    }
                }
                Ok(())
            }

            Commands::Plugin(cmd) => match cmd {
                PluginCommands::List => {
                    let config = load_config(&config_path).unwrap_or_else(|_| default_config());
                    let skills_dir = &config.plugins.skills_dir;
                    let plugins_dir = &config.plugins.plugins_dir;
                    let skills = SkillLoader::load_from_dir(skills_dir);
                    let plugins = PluginLoader::load_from_dir(plugins_dir);

                    if skills.is_empty() && plugins.is_empty() {
                        println!("No skills or plugins installed.");
                        println!(
                            "\nInstall a skill with:   unly plugin install <path-to-skill-dir>"
                        );
                    } else {
                        print_ext_table(
                            "Skills",
                            skills_dir,
                            skills.iter().map(|s| {
                                (s.meta.name.clone(), s.enabled, s.meta.description.clone())
                            }),
                            "Skills: none installed.",
                        );
                        println!();
                        print_ext_table(
                            "Plugins",
                            plugins_dir,
                            plugins.iter().map(|p| {
                                (p.meta.name.clone(), p.enabled, p.meta.description.clone())
                            }),
                            "Plugins: none installed.",
                        );
                    }
                    Ok(())
                }
                PluginCommands::Install { path } => {
                    let config = load_config(&config_path).unwrap_or_else(|_| default_config());
                    let skills_dir = &config.plugins.skills_dir;
                    let src = std::fs::canonicalize(&path)
                        .with_context(|| format!("path does not exist: {}", path.display()))?;
                    match SkillLoader::install(&src, skills_dir) {
                        Ok(name) => {
                            println!("Skill '{}' installed successfully.", name);
                            println!("Skills directory: {}", skills_dir.display());
                        }
                        Err(e) => bail!("{}", e),
                    }
                    Ok(())
                }
                PluginCommands::Remove { id } => {
                    let config = load_config(&config_path).unwrap_or_else(|_| default_config());
                    let skills_dir = &config.plugins.skills_dir;
                    match SkillLoader::remove(&id, skills_dir) {
                        Ok(()) => println!("Skill '{}' removed.", id),
                        Err(e) => bail!("{}", e),
                    }
                    Ok(())
                }
                PluginCommands::Enable { id } => {
                    let config = load_config(&config_path).unwrap_or_else(|_| default_config());
                    let skills_dir = &config.plugins.skills_dir;
                    match SkillLoader::enable(&id, skills_dir) {
                        Ok(()) => println!("Skill '{}' enabled.", id),
                        Err(e) => bail!("{}", e),
                    }
                    Ok(())
                }
                PluginCommands::Disable { id } => {
                    let config = load_config(&config_path).unwrap_or_else(|_| default_config());
                    let skills_dir = &config.plugins.skills_dir;
                    match SkillLoader::disable(&id, skills_dir) {
                        Ok(()) => println!("Skill '{}' disabled.", id),
                        Err(e) => bail!("{}", e),
                    }
                    Ok(())
                }
            },

            Commands::Job(cmd) => {
                init_logging("info", false);
                let config = load_config(&config_path).context("loading config")?;
                let db = Database::connect_with_config(&config.database)
                    .await
                    .context("connecting to database")?;

                match cmd {
                    JobCommands::List => {
                        let repo = unly_db::repo::job::JobRepo::new(db.conn());
                        let jobs = repo.list_enabled().await?;
                        if jobs.is_empty() {
                            println!("No jobs defined.");
                        } else {
                            println!("{:<36} {:<20} {:<10}", "ID", "Name", "Status");
                            println!("{}", "-".repeat(70));
                            for j in &jobs {
                                println!("{:<36} {:<20} {:<10}", j.id, j.name, j.status);
                            }
                        }
                    }
                    JobCommands::Run { id } => {
                        println!("Triggering job: {} (not yet fully implemented via CLI)", id);
                    }
                    JobCommands::Enable { id: _ } | JobCommands::Disable { id: _ } => {
                        println!("Update job enabled state via the database or config.");
                    }
                }
                Ok(())
            }

            Commands::Migrate => {
                init_logging("info", false);
                let config = load_config(&config_path).context("loading config")?;
                let db = Database::connect_with_config(&config.database)
                    .await
                    .context("connecting to database")?;
                db.migrate().await.context("running migrations")?;
                println!("Migrations complete");
                Ok(())
            }

            Commands::Audit { n } => {
                init_logging("info", false);
                let config = load_config(&config_path).context("loading config")?;
                let db = Database::connect_with_config(&config.database)
                    .await
                    .context("connecting to database")?;
                let repo = unly_db::repo::audit::AuditRepo::new(db.conn());
                let rows = repo.list_recent(n).await?;
                if rows.is_empty() {
                    println!("No audit log entries.");
                } else {
                    println!(
                        "{:<24} {:<20} {:<30} {:<10}",
                        "Time", "Event", "Action", "Outcome"
                    );
                    println!("{}", "-".repeat(90));
                    for r in &rows {
                        println!(
                            "{:<24} {:<20} {:<30} {:<10}",
                            r.created_at.format("%Y-%m-%d %H:%M:%S"),
                            r.event_type,
                            r.action,
                            r.outcome
                        );
                    }
                }
                Ok(())
            }

            Commands::Memory(cmd) => {
                init_logging("info", false);
                let config = load_config(&config_path).context("loading config")?;
                let db = Database::connect_with_config(&config.database)
                    .await
                    .context("connecting to database")?;

                match cmd {
                    MemoryCommands::List { scope, n } => {
                        let parts: Vec<&str> = scope.splitn(2, ':').collect();
                        if parts.len() != 2 {
                            bail!("scope must be in format 'type:id' (e.g. 'chat:uuid')");
                        }
                        let repo = unly_db::repo::memory::MemoryRepo::new(db.conn());
                        let entries = repo.list_by_scope(parts[0], parts[1]).await?;
                        if entries.is_empty() {
                            println!("No memory entries for scope: {}", scope);
                        } else {
                            for e in entries.iter().take(n as usize) {
                                println!(
                                    "[{}] {} - {}",
                                    e.created_at.format("%Y-%m-%d %H:%M"),
                                    e.id,
                                    &e.content[..e.content.len().min(100)]
                                );
                            }
                        }
                    }
                    MemoryCommands::Prune => {
                        let repo = unly_db::repo::memory::MemoryRepo::new(db.conn());
                        let deleted = repo.delete_expired().await?;
                        println!("Pruned {} expired memory entries", deleted);
                    }
                }
                Ok(())
            }

            Commands::InitConfig { output } => {
                let out = output.unwrap_or_else(workspace::default_config_path);
                if out.exists() {
                    bail!(
                        "file already exists: {}. Remove it first or choose a different path.",
                        out.display()
                    );
                }
                // Ensure workspace directory exists.
                workspace::ensure_workspace()?;
                let config = default_config();
                let toml_content =
                    toml::to_string_pretty(&config).context("serializing default config")?;
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&out, &toml_content)
                    .with_context(|| format!("writing config to {}", out.display()))?;
                println!("Default configuration written to: {}", out.display());
                println!("\nEdit this file and set at minimum:");
                println!("  - telegram.bot_token");
                println!("  - telegram.admin_user_ids");
                println!("\nThen run: unly provider-login copilot");
                Ok(())
            }

            Commands::Uninstall { skip } => run_uninstall_wizard(skip, &config_path).await,

            Commands::Update { check } => {
                if check {
                    match self_update::check_update().await {
                        Ok(Some((current, latest, url))) => {
                            println!("Update available: v{} → v{}", current, latest);
                            println!("Release: {}", url);
                            println!("\nRun `unly update` to install.");
                        }
                        Ok(None) => {
                            println!("Already up-to-date (v{}).", env!("CARGO_PKG_VERSION"));
                        }
                        Err(e) => bail!("update check failed: {}", e),
                    }
                } else {
                    self_update::perform_update()
                        .await
                        .context("self-update failed")?;
                }
                Ok(())
            }
        }
    }
}

// ── Setup Wizard ─────────────────────────────────────────────────────────────

async fn run_setup_wizard(config_path: &PathBuf) -> Result<()> {
    let theme = ColorfulTheme::default();

    println!("\nUnly Setup Wizard");
    println!("{}", "=".repeat(50));
    println!("This wizard configures your Unly agent platform.\n");

    if config_path.exists() {
        let overwrite = Confirm::with_theme(&theme)
            .with_prompt(format!(
                "Config already exists at {}. Overwrite and reset workspace data?",
                config_path.display()
            ))
            .default(false)
            .interact()?;
        if !overwrite {
            println!("Setup cancelled. Existing config unchanged.");
            return Ok(());
        }
        let workspace_dir = workspace::workspace_dir();
        if workspace_dir.exists() {
            std::fs::remove_dir_all(&workspace_dir).with_context(|| {
                format!(
                    "removing existing workspace for overwrite: {}",
                    workspace_dir.display()
                )
            })?;
            println!("Existing workspace removed: {}", workspace_dir.display());
        } else if config_path.exists() {
            remove_path_if_exists(config_path)
                .with_context(|| format!("removing existing config {}", config_path.display()))?;
        }
    }

    // Ensure the workspace directory exists.
    workspace::ensure_workspace()?;

    // ── Telegram ────────────────────────────────────────────────────────────
    println!("\n[1/4] Telegram Bot");
    println!("Create a bot at https://t.me/BotFather and paste the token below.");

    let bot_token: String = Input::with_theme(&theme)
        .with_prompt("Telegram bot token")
        .interact_text()?;

    let admin_id_str: String = Input::with_theme(&theme)
        .with_prompt("Your Telegram user ID (get it from @userinfobot)")
        .validate_with(|s: &String| {
            s.trim()
                .parse::<i64>()
                .map(|_| ())
                .map_err(|_| "Please enter a numeric user ID")
        })
        .interact_text()?;
    let admin_id: i64 = admin_id_str.trim().parse()?;

    // ── LLM Provider ────────────────────────────────────────────────────────
    println!("\n[2/4] LLM Provider");
    let provider_choices = &[
        "GitHub Copilot (requires subscription, login via device flow)",
        "OpenAI-compatible API (OpenAI, Azure, Together AI, ...)",
        "Local / embedded (Ollama running on localhost:11434)",
    ];
    let provider_idx = Select::with_theme(&theme)
        .with_prompt("Choose an LLM provider")
        .items(provider_choices)
        .default(0)
        .interact()?;

    // ── Database ────────────────────────────────────────────────────────────
    println!("\n[3/4] Database");
    let db_choices = &["SQLite (embedded, zero-config)", "PostgreSQL"];
    let db_idx = Select::with_theme(&theme)
        .with_prompt("Choose a database backend")
        .items(db_choices)
        .default(0)
        .interact()?;

    let postgres_url: Option<String> = if db_idx == 1 {
        let url: String = Input::with_theme(&theme)
            .with_prompt("PostgreSQL connection URL")
            .default("postgresql://postgres:password@localhost:5432/unly".to_string())
            .interact_text()?;
        Some(url)
    } else {
        None
    };

    let full_access = Confirm::with_theme(&theme)
        .with_prompt("Give the agent full tool access (no approval prompts)?")
        .default(false)
        .interact()?;

    // ── Build config ─────────────────────────────────────────────────────────
    println!("\n[4/4] Writing configuration...");

    let mut config = default_config();
    config.telegram.bot_token = bot_token;
    config.telegram.admin_user_ids = vec![admin_id];

    match provider_idx {
        0 => {
            // Copilot - already the default
            config.providers.copilot.enabled = true;
            config.providers.default_provider = "copilot".to_string();
        }
        1 => {
            // OpenAI-compatible
            let base_url: String = Input::with_theme(&theme)
                .with_prompt("API base URL")
                .default("https://api.openai.com/v1".to_string())
                .interact_text()?;
            let api_key: String = Input::with_theme(&theme)
                .with_prompt("API key")
                .interact_text()?;
            let model: String = Input::with_theme(&theme)
                .with_prompt("Default model ID")
                .default("gpt-4o".to_string())
                .interact_text()?;

            config.providers.copilot.enabled = false;
            config
                .providers
                .openai_compatible
                .push(unly_config::OpenAiCompatConfig {
                    name: "openai".to_string(),
                    enabled: true,
                    base_url,
                    api_key,
                    models: vec![model.clone()],
                });
            config.providers.default_provider = "openai".to_string();
            config.providers.default_model = model;
        }
        2 => {
            // Local / Ollama
            let model: String = Input::with_theme(&theme)
                .with_prompt("Ollama model name")
                .default("llama3.2".to_string())
                .interact_text()?;

            config.providers.copilot.enabled = false;
            config
                .providers
                .openai_compatible
                .push(unly_config::OpenAiCompatConfig {
                    name: "local".to_string(),
                    enabled: true,
                    base_url: "http://localhost:11434/v1".to_string(),
                    api_key: "ollama".to_string(),
                    models: vec![model.clone()],
                });
            config.providers.default_provider = "local".to_string();
            config.providers.default_model = model;
        }
        _ => unreachable!(),
    }

    if db_idx == 1 {
        config.database.db_type = unly_config::DbType::Postgres;
        config.database.postgres_url = postgres_url;
    }

    if full_access {
        config.tools.require_approval_for_privileged = false;
        config.tools.require_approval_for_dangerous = false;
        config.tools.shell_allowlist = vec![r"(?s)^.*$".to_string()];
    }

    // Write config file.
    let toml_content = toml::to_string_pretty(&config).context("serializing config")?;
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, &toml_content)
        .with_context(|| format!("writing config to {}", config_path.display()))?;

    // Write default identity files.
    let id_path = workspace::identity_path();
    if !id_path.exists() {
        let _ = std::fs::write(&id_path, workspace::DEFAULT_IDENTITY);
        println!("Identity file created: {}", id_path.display());
    }
    let soul_path = workspace::soul_path();
    if !soul_path.exists() {
        let _ = std::fs::write(&soul_path, workspace::DEFAULT_SOUL);
        println!("Soul file created: {}", soul_path.display());
    }
    let tools_path = workspace::tools_path();
    if !tools_path.exists() {
        let _ = std::fs::write(&tools_path, workspace::DEFAULT_TOOLS);
        println!("Tools file created: {}", tools_path.display());
    }
    let memory_path = workspace::memory_index_path();
    if !memory_path.exists() {
        let _ = std::fs::write(&memory_path, workspace::DEFAULT_MEMORY);
        println!("Memory index created: {}", memory_path.display());
    }
    let memory_today = workspace::memory_today_path();
    if !memory_today.exists() {
        if let Some(parent) = memory_today.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&memory_today, workspace::DEFAULT_MEMORY_TODAY);
        println!("Today memory file created: {}", memory_today.display());
    }
    let boot_path = workspace::boot_path();
    if !boot_path.exists() {
        let _ = std::fs::write(&boot_path, workspace::DEFAULT_BOOT);
        println!("Boot file created: {}", boot_path.display());
    }

    if provider_idx == 0 {
        let cp = CopilotProvider::new(
            config.providers.copilot.github_client_id.clone(),
            config.providers.copilot.token_cache_path.clone(),
            config.providers.copilot.copilot_api_url.clone(),
        );
        println!("\nStarting GitHub Copilot authentication...");
        let state = cp
            .start_device_flow()
            .await
            .map_err(|e| anyhow::anyhow!("device flow start failed: {}", e))?;
        println!("Open this URL: {}", state.verification_uri);
        println!("Enter code: {}", state.user_code);
        println!("Waiting for authorization...");
        let poll_interval = Duration::from_secs(state.interval.max(5));
        let timeout = Duration::from_secs(state.expires_in);
        let start = std::time::Instant::now();
        loop {
            if start.elapsed() > timeout {
                bail!("device flow timed out");
            }
            tokio::time::sleep(poll_interval).await;
            match cp.poll_device_flow(&state).await {
                Ok(DevicePollResult::Authorized) => {
                    println!("Authenticated with GitHub Copilot.");
                    println!(
                        "Token cached at: {}",
                        config.providers.copilot.token_cache_path.display()
                    );
                    break;
                }
                Ok(DevicePollResult::Pending) => {
                    print!(".");
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
                Ok(DevicePollResult::SlowDown) => {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                Ok(DevicePollResult::Denied) => bail!("authorization was denied"),
                Ok(DevicePollResult::Expired) => bail!("device code expired - please try again"),
                Ok(DevicePollResult::Error(e)) => bail!("authorization error: {}", e),
                Err(e) => bail!("polling error: {}", e),
            }
        }
    }

    println!("\nConfiguration written to: {}", config_path.display());
    println!("\nNext steps:");
    println!("  1. Run: unly start");

    Ok(())
}

async fn run_uninstall_wizard(skip: bool, config_path: &Path) -> Result<()> {
    let theme = ColorfulTheme::default();
    let workspace_dir = workspace::workspace_dir();
    let binary_path = cargo_unly_binary_path();
    let custom_config_outside_workspace =
        config_path.exists() && !config_path.starts_with(&workspace_dir);

    println!("\nUnly Uninstall");
    println!("{}", "=".repeat(50));
    println!(
        "This will permanently delete the full Unly workspace:\n  {}",
        workspace_dir.display()
    );
    if custom_config_outside_workspace {
        println!("And custom config file:\n  {}", config_path.display());
    }
    if binary_path.exists() {
        println!("And binary:\n  {}", binary_path.display());
    }

    let any_target_exists =
        workspace_dir.exists() || custom_config_outside_workspace || binary_path.exists();
    if !any_target_exists {
        println!("Nothing to remove.");
        return Ok(());
    }

    if !skip {
        let confirm_first = Confirm::with_theme(&theme)
            .with_prompt("Are you sure you want to delete selected Unly data?")
            .default(false)
            .interact()?;
        if !confirm_first {
            println!("Uninstall cancelled.");
            return Ok(());
        }

        println!("Waiting 10 seconds before final confirmation...");
        std::thread::sleep(Duration::from_secs(10));

        let confirm_second = Confirm::with_theme(&theme)
            .with_prompt("This action is irreversible. Confirm deletion again?")
            .default(false)
            .interact()?;
        if !confirm_second {
            println!("Uninstall cancelled.");
            return Ok(());
        }
    }

    if workspace_dir.exists() {
        std::fs::remove_dir_all(&workspace_dir)
            .with_context(|| format!("removing workspace {}", workspace_dir.display()))?;
        println!("Unly workspace removed: {}", workspace_dir.display());
    }
    if custom_config_outside_workspace {
        remove_path_if_exists(config_path)
            .with_context(|| format!("removing custom config {}", config_path.display()))?;
        println!("Custom config removed: {}", config_path.display());
    }
    if binary_path.exists() {
        remove_path_if_exists(&binary_path)
            .with_context(|| format!("removing binary {}", binary_path.display()))?;
        println!("Binary removed: {}", binary_path.display());
    }

    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn cargo_unly_binary_path() -> PathBuf {
    let cargo_home = std::env::var("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".cargo")
        });
    cargo_home.join("bin").join("unly")
}

/// Print a table of skills or plugins to stdout.
///
/// `title` is shown as the section heading with the directory path.
/// `rows` is an iterator of `(name, enabled, description)` tuples.
fn print_ext_table(
    title: &str,
    dir: &std::path::Path,
    rows: impl Iterator<Item = (String, bool, String)>,
    none_msg: &str,
) {
    let rows: Vec<_> = rows.collect();
    if rows.is_empty() {
        println!("{}", none_msg);
        return;
    }
    println!("{} ({})", title, dir.display());
    println!("{:<30} {:<10} Description", "Name", "Status");
    println!("{}", "-".repeat(80));
    for (name, enabled, description) in &rows {
        let status = if *enabled { "enabled" } else { "disabled" };
        println!("{:<30} {:<10} {}", name, status, description);
    }
}
