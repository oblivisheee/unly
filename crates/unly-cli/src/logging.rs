use std::path::Path;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init_logging(level: &str, json: bool) {
    init_logging_with_file(level, json, None);
}

pub fn init_logging_with_file(level: &str, json: bool, log_file: Option<&Path>) {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    if let Some(path) = log_file {
        // Ensure parent directory exists.
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(f) => f,
            Err(e) => {
                eprintln!("warning: failed to open log file {}: {}", path.display(), e);
                // Fall back to stderr-only logging.
                return init_logging_stderr(level, json, filter);
            }
        };

        if json {
            let subscriber = tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json().with_writer(file));
            tracing::subscriber::set_global_default(subscriber)
                .expect("failed to set tracing subscriber");
        } else {
            let subscriber = tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().with_target(true).with_writer(file));
            tracing::subscriber::set_global_default(subscriber)
                .expect("failed to set tracing subscriber");
        }
    } else {
        init_logging_stderr(level, json, filter);
    }
}

fn init_logging_stderr(level: &str, json: bool, filter: EnvFilter) {
    let _ = level; // already embedded in filter
    if json {
        let subscriber = tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json());
        tracing::subscriber::set_global_default(subscriber)
            .expect("failed to set tracing subscriber");
    } else {
        let subscriber = tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().with_target(true));
        tracing::subscriber::set_global_default(subscriber)
            .expect("failed to set tracing subscriber");
    }
}
