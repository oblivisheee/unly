use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init_logging(level: &str, json: bool) {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

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
