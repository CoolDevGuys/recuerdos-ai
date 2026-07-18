mod bootstrap;
mod consolidation;
mod identity;
mod memories;
mod providers;
mod shared;
mod understanding;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "recordagent",
    version,
    about = "Long-term memory service for AI agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the daemon.
    Serve {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Write a default recordagent.toml and create its data directory.
    Init {
        #[arg(long, default_value = "recordagent.toml")]
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Serve { config } => run_serve(config.as_deref()).await,
        Command::Init { config } => run_init(&config),
    };

    if let Err(message) = result {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

async fn run_serve(config_path: Option<&Path>) -> Result<(), String> {
    bootstrap::server::init_tracing();

    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;

    bootstrap::server::serve(&config.server.host, config.server.port)
        .await
        .map_err(|e| format!("server error: {e}"))
}

fn run_init(config_path: &PathBuf) -> Result<(), String> {
    const EXAMPLE_CONFIG: &str = include_str!("../recordagent.example.toml");

    if config_path.exists() {
        return Err(format!(
            "{} already exists, refusing to overwrite",
            config_path.display()
        ));
    }

    std::fs::write(config_path, EXAMPLE_CONFIG)
        .map_err(|e| format!("failed to write {}: {e}", config_path.display()))?;

    // Prove the file we just wrote actually loads before declaring success.
    let config = bootstrap::config::AppConfig::load(Some(config_path))
        .map_err(|e| format!("wrote {} but it fails to load: {e}", config_path.display()))?;

    let data_dir = config.data_dir();
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("failed to create data dir {}: {e}", data_dir.display()))?;

    println!("wrote {}", config_path.display());
    println!("created data directory {}", data_dir.display());
    Ok(())
}
