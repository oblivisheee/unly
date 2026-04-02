use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

use unly_audit::AuditLogger;
use unly_config::{default_config, load_config};
use unly_db::Database;
use unly_providers::copilot::CopilotProvider;
use unly_telegram::{SessionStore, TelegramBot};

use crate::{
    logging::init_logging,
    service::{build_providers, build_runtime, build_tools},
};

/// Unly — self-hosted personal AI agent platform.
#[derive(Parser)]
#[command(name = "unly", version, about = "Unly personal AI agent platform")]
pub struct Cli {
    /// Path to the configuration file.
    #[arg(
        short,
        long,
        default_value = "config.toml",
        env = "UNLY_CONFIG",
        global = true
    )]
    pub config: PathBuf,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the Telegram bot and all subsystems.
    Start,

    /// Run first-run onboarding wizard.
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
        /// Output file path.
        #[arg(default_value = "config.toml")]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum PluginCommands {
    /// List installed plugins.
    List,
    /// Enable a plugin.
    Enable { id: String },
    /// Disable a plugin.
    Disable { id: String },
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
        match self.command {
            Commands::Start => {
                let config = load_config(&self.config)
                    .with_context(|| format!("loading config from {}", self.config.display()))?;

                init_logging(&config.logging.level, config.logging.json);
                info!("starting unly agent platform v{}", env!("CARGO_PKG_VERSION"));

                // Connect to database.
                let db = Database::connect(
                    &config.database.path,
                    config.database.max_connections,
                    config.database.auto_migrate,
                )
                .await
                .context("failed to connect to database")?;

                info!("database connected");

                // Build subsystems.
                let audit = Arc::new(AuditLogger::new(db.clone()));
                let providers = build_providers(&config).await?;
                let tools = build_tools(&config);
                let runtime = build_runtime(&config, providers.clone(), tools, Some(audit.clone()));
                let sessions = SessionStore::new();

                let config_arc = Arc::new(config);
                let bot = Arc::new(TelegramBot::new(
                    config_arc.clone(),
                    sessions,
                    runtime,
                    providers,
                    db,
                    audit.clone(),
                ));

                info!("all subsystems initialized — starting Telegram bot");
                audit.success("startup", "system", "start");

                bot.start().await;

                Ok(())
            }

            Commands::Setup => {
                println!("🧙 Unly First-Run Setup Wizard");
                println!("================================\n");
                println!(
                    "This wizard will guide you through configuring the Unly agent platform.\n"
                );

                let config_path = &self.config;
                if config_path.exists() {
                    println!(
                        "⚠️  Configuration file already exists at: {}",
                        config_path.display()
                    );
                    println!("Run with a different path or delete the existing file to start fresh.\n");
                }

                println!("📋 Required configuration steps:");
                println!("  1. Set TELEGRAM_BOT_TOKEN env var (from @BotFather)");
                println!("  2. Set TELEGRAM_ADMIN_USER_IDS env var (your Telegram user ID)");
                println!("  3. Authenticate with GitHub Copilot: `unly provider-login copilot`");
                println!("  4. Generate config file: `unly init-config`");
                println!("  5. Start the bot: `unly start`\n");

                println!("📖 See docs/setup.md for detailed instructions.\n");

                // Generate default config if requested.
                let config = default_config();
                let toml_content = toml::to_string_pretty(&config)
                    .context("failed to serialize default config")?;
                println!("Default config template:\n");
                println!("{}", &toml_content[..toml_content.len().min(500)]);
                println!("...\n");
                println!("Run `unly init-config` to write the full default config file.");

                Ok(())
            }

            Commands::Validate => {
                match load_config(&self.config) {
                    Ok(config) => {
                        println!("✅ Configuration is valid");
                        println!("   Telegram bot token: configured");
                        println!(
                            "   Admin user IDs: {:?}",
                            config.telegram.admin_user_ids
                        );
                        println!(
                            "   Default provider: {}",
                            config.providers.default_provider
                        );
                        println!("   Default model: {}", config.providers.default_model);
                        println!("   Database: {}", config.database.path.display());
                    }
                    Err(e) => {
                        eprintln!("❌ Configuration invalid: {}", e);
                        std::process::exit(1);
                    }
                }
                Ok(())
            }

            Commands::Doctor => {
                init_logging("info", false);

                let config = match load_config(&self.config) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("❌ Config: {}", e);
                        std::process::exit(1);
                    }
                };

                println!("🔍 Unly Diagnostics\n");

                // Database check.
                match Database::connect(&config.database.path, 1, false).await {
                    Ok(db) => {
                        match db.health_check().await {
                            Ok(_) => println!("✅ Database: ok ({})", config.database.path.display()),
                            Err(e) => println!("❌ Database: {}", e),
                        }
                    }
                    Err(e) => println!("❌ Database: {}", e),
                }

                // Provider checks.
                let providers = build_providers(&config).await?;
                let reports = providers.health_all().await;
                for r in &reports {
                    let icon = match r.status {
                        unly_core::types::HealthStatus::Healthy => "✅",
                        unly_core::types::HealthStatus::Degraded => "⚠️",
                        unly_core::types::HealthStatus::Unhealthy => "❌",
                        unly_core::types::HealthStatus::Unknown => "❓",
                    };
                    println!(
                        "{} Provider {}: {}",
                        icon,
                        r.name,
                        r.message.as_deref().unwrap_or("ok")
                    );
                }

                // Tool check.
                let tools = build_tools(&config);
                let tool_count = tools.list_schemas().len();
                println!("✅ Tools: {} registered", tool_count);

                println!("\n✅ Diagnostics complete");
                Ok(())
            }

            Commands::ProviderLogin { provider } => {
                init_logging("info", false);

                let config = load_config(&self.config).or_else(|_| {
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

                        println!("🔑 Authenticating with GitHub Copilot...\n");

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
                        let poll_interval =
                            Duration::from_secs(state.interval.max(5));
                        let timeout = Duration::from_secs(state.expires_in);
                        let start = std::time::Instant::now();

                        loop {
                            if start.elapsed() > timeout {
                                bail!("device flow timed out");
                            }
                            tokio::time::sleep(poll_interval).await;
                            match cp.poll_device_flow(&state).await {
                                Ok(unly_providers::copilot::DevicePollResult::Authorized) => {
                                    println!("✅ Successfully authenticated with GitHub Copilot!");
                                    println!(
                                        "   Token cached at: {}",
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
                                    bail!("device code expired — please try again");
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
                let config = load_config(&self.config).unwrap_or_else(|_| default_config());
                let providers = build_providers(&config).await?;
                println!("Provider Status:\n");
                for name in providers.provider_names() {
                    if let Some(p) = providers.get(&name) {
                        let report = p.health().await;
                        let icon = match report.status {
                            unly_core::types::HealthStatus::Healthy => "✅",
                            unly_core::types::HealthStatus::Degraded => "⚠️",
                            _ => "❌",
                        };
                        println!(
                            "  {} {} — {}",
                            icon,
                            name,
                            report.message.as_deref().unwrap_or("ok")
                        );
                    }
                }
                Ok(())
            }

            Commands::Plugin(cmd) => match cmd {
                PluginCommands::List => {
                    println!("Plugin management via CLI: use unly plugin list");
                    println!("No plugins currently installed.");
                    println!("\nTo install a plugin, see docs/plugins.md");
                    Ok(())
                }
                PluginCommands::Enable { id } => {
                    println!("Plugin enable: {} (update config.toml plugins.enabled)", id);
                    Ok(())
                }
                PluginCommands::Disable { id } => {
                    println!("Plugin disable: {} (update config.toml plugins.disabled)", id);
                    Ok(())
                }
            },

            Commands::Job(cmd) => {
                init_logging("info", false);
                let config = load_config(&self.config).context("loading config")?;
                let db = Database::connect(&config.database.path, 1, false)
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
                let config = load_config(&self.config).context("loading config")?;
                let db = Database::connect(
                    &config.database.path,
                    config.database.max_connections,
                    false,
                )
                .await
                .context("connecting to database")?;
                db.migrate().await.context("running migrations")?;
                println!("✅ Migrations complete");
                Ok(())
            }

            Commands::Audit { n } => {
                init_logging("info", false);
                let config = load_config(&self.config).context("loading config")?;
                let db = Database::connect(&config.database.path, 1, false)
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
                let config = load_config(&self.config).context("loading config")?;
                let db = Database::connect(&config.database.path, 1, false)
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
                                    "[{}] {} — {}",
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
                        println!("✅ Pruned {} expired memory entries", deleted);
                    }
                }
                Ok(())
            }

            Commands::InitConfig { output } => {
                if output.exists() {
                    bail!(
                        "file already exists: {}. Remove it first or choose a different path.",
                        output.display()
                    );
                }
                let config = default_config();
                let toml_content =
                    toml::to_string_pretty(&config).context("serializing default config")?;
                std::fs::write(&output, &toml_content)
                    .with_context(|| format!("writing config to {}", output.display()))?;
                println!(
                    "✅ Default configuration written to: {}",
                    output.display()
                );
                println!("\n⚠️  Edit this file and set at minimum:");
                println!("   - telegram.bot_token");
                println!("   - telegram.admin_user_ids");
                println!("\nThen run: unly provider-login copilot");
                Ok(())
            }
        }
    }
}
