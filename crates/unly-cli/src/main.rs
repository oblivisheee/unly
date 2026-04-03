//! CLI and service entrypoint for the unly agent platform.

mod commands;
mod logging;
mod service;
mod update;

use anyhow::Result;
use clap::Parser;
use commands::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    cli.run().await
}
