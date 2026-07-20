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
    /// Serve MCP over stdio, for an MCP client to spawn.
    ///
    /// Forwards to a running daemon; set RECORDAGENT_API_KEY (and
    /// RECORDAGENT_URL if it isn't on localhost:7070).
    Mcp {
        /// Recorded as the source of memories this client saves.
        #[arg(long, default_value = "mcp")]
        client: String,
    },
    /// Score retrieval quality against the committed eval set.
    ///
    /// Hidden because it is a development and CI tool, not something an
    /// operator runs. Needs the embedding model on disk — the scores are
    /// meaningless without the real one.
    #[command(hide = true)]
    Eval {
        #[arg(long, default_value = "eval/cases.toml")]
        cases: PathBuf,
        /// Compare against this baseline and fail on a regression.
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// How many percentage points recall@5 may drop before failing.
        #[arg(long, default_value_t = 5.0)]
        max_drop: f64,
        /// Record the current scores as the new baseline instead.
        #[arg(long)]
        write_baseline: Option<PathBuf>,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Download the local embedding model into the cache directory.
    ///
    /// Run at image build time so a container never downloads at
    /// runtime, or before taking a host offline.
    WarmModels {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Manage users.
    User {
        #[command(subcommand)]
        command: identity::infrastructure::cli::UserCommand,
        #[arg(long, global = true)]
        config: Option<PathBuf>,
    },
    /// Manage API keys.
    Key {
        #[command(subcommand)]
        command: identity::infrastructure::cli::KeyCommand,
        #[arg(long, global = true)]
        config: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Serve { config } => run_serve(config.as_deref()).await,
        Command::Init { config } => run_init(&config),
        Command::Mcp { client } => run_mcp(&client).await,
        Command::Eval {
            cases,
            baseline,
            max_drop,
            write_baseline,
            config,
        } => run_eval(
            &cases,
            baseline.as_deref(),
            max_drop,
            write_baseline.as_deref(),
            config.as_deref(),
        ),
        Command::WarmModels { config } => run_warm_models(config.as_deref()),
        Command::User { command, config } => run_user(command, config.as_deref()),
        Command::Key { command, config } => run_key(command, config.as_deref()),
    };

    if let Err(message) = result {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

async fn run_serve(config_path: Option<&Path>) -> Result<(), String> {
    bootstrap::server::init_tracing();

    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;

    // Both contexts share one database handle: SQLite has a single
    // writer, and two handles would mean two connections contending for
    // the same file rather than queueing behind one mutex.
    let database = bootstrap::wiring::open_database(&config).map_err(|e| e.to_string())?;
    let identity = bootstrap::wiring::Identity::from_database(std::sync::Arc::clone(&database))
        .map_err(|e| e.to_string())?;
    let memories =
        bootstrap::memories_wiring::Memories::build(&config, std::sync::Arc::clone(&database))
            .map_err(|e| e.to_string())?;
    let understanding =
        bootstrap::understanding_wiring::Understanding::build(&config, database, &memories)
            .map_err(|e| e.to_string())?;

    let identity = std::sync::Arc::new(identity);
    let understanding = std::sync::Arc::new(understanding);

    // Workers start before the listener binds. Anything left pending by
    // the previous process is already draining by the time the first new
    // request can arrive.
    let workers = understanding::infrastructure::ingest_workers::IngestWorkers {
        queue: std::sync::Arc::clone(&understanding.queue),
        pipeline: std::sync::Arc::clone(&understanding.pipeline),
        users: std::sync::Arc::new(
            identity::application::background_user_resolver::BackgroundUserResolver::new(
                std::sync::Arc::clone(&identity.users),
            ),
        ),
        clock: std::sync::Arc::clone(&identity.clock),
        max_attempts: config.understanding.max_attempts,
        wake: std::sync::Arc::clone(&understanding.wake),
    }
    .start(config.understanding.workers)
    .await
    .map_err(|e| e.to_string())?;

    let state = bootstrap::state::AppState {
        identity,
        memories: std::sync::Arc::new(memories),
        understanding,
        auth_mode: bootstrap::state::AuthMode::from_config(&config),
    };

    let outcome = bootstrap::server::serve(&config.server.host, config.server.port, state).await;

    // After the listener drains, so a job enqueued by the last in-flight
    // request still gets picked up rather than waiting for a restart.
    workers.shutdown().await;

    outcome.map_err(|e| format!("server error: {e}"))
}

async fn run_mcp(client_name: &str) -> Result<(), String> {
    // Deliberately no tracing init: stdout is the MCP protocol channel,
    // and the subscriber's default writer is stdout. The shim writes
    // diagnostics to stderr itself.
    memories::infrastructure::mcp::stdio_shim::serve_stdio(client_name)
        .await
        .map_err(|e| e.to_string())
}

fn run_eval(
    cases: &Path,
    baseline: Option<&Path>,
    max_drop: f64,
    write_baseline: Option<&Path>,
    config_path: Option<&Path>,
) -> Result<(), String> {
    // The model cache is the one thing the eval needs from config; its
    // data directory is a throwaway it creates itself, so that running
    // the eval can never touch a real store.
    let cache_dir = bootstrap::config::AppConfig::load(config_path)
        .map(|config| config.model_cache_dir())
        .map_err(|e| e.to_string())?;

    let report = bootstrap::eval::run(cases, Some(&cache_dir)).map_err(|e| e.to_string())?;

    if let Some(path) = write_baseline {
        return bootstrap::eval::write_baseline(&report, path).map_err(|e| e.to_string());
    }
    if let Some(path) = baseline {
        return bootstrap::eval::compare(&report, path, max_drop).map_err(|e| e.to_string());
    }
    Ok(())
}

fn run_warm_models(config_path: Option<&Path>) -> Result<(), String> {
    use memories::domain::embedder::Embedder;

    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;
    let cache_dir = config.model_cache_dir();

    println!(
        "downloading embedding model {} into {}",
        config.embeddings.model,
        cache_dir.display()
    );

    let embedder =
        providers::infrastructure::embeddings::fastembed_embedder::FastembedEmbedder::load(
            &config.embeddings.model,
            cache_dir,
        )
        .map_err(|e| e.to_string())?;

    // Embedding something proves the model actually runs, not just that
    // the files downloaded — a truncated download would otherwise only
    // surface on the first real request.
    embedder
        .embed(&["warm".to_string()])
        .map_err(|e| e.to_string())?;

    println!("model ready ({} dimensions)", embedder.dimensions());
    Ok(())
}

fn run_user(
    command: identity::infrastructure::cli::UserCommand,
    config_path: Option<&Path>,
) -> Result<(), String> {
    let identity = build_identity(config_path)?;
    identity::infrastructure::cli::run_user_command(command, &identity).map_err(|e| e.to_string())
}

fn run_key(
    command: identity::infrastructure::cli::KeyCommand,
    config_path: Option<&Path>,
) -> Result<(), String> {
    let identity = build_identity(config_path)?;
    identity::infrastructure::cli::run_key_command(command, &identity).map_err(|e| e.to_string())
}

fn build_identity(config_path: Option<&Path>) -> Result<bootstrap::wiring::Identity, String> {
    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;
    bootstrap::wiring::Identity::build(&config).map_err(|e| e.to_string())
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
