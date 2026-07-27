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
    name = "recuerdos-ai",
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
    /// Write a default recuerdos-ai.toml and create its data directory.
    Init {
        #[arg(long, default_value = "recuerdos-ai.toml")]
        config: PathBuf,
    },
    /// Print the resolved configuration — providers, models, transports.
    ///
    /// Shows the effective values after defaults, the file and
    /// `RECUERDOS_AI_*` env vars are merged, so it answers "which provider
    /// am I actually using". Prints no secrets: for an API key it shows
    /// only the env var's name and whether it is set, never the value.
    Config {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Serve MCP over stdio, for an MCP client to spawn.
    ///
    /// Forwards to a running daemon; set RECUERDOS_AI_API_KEY (and
    /// RECUERDOS_AI_URL if it isn't on localhost:7070).
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
    /// Re-embed every memory under the current [embeddings] model, in place.
    ///
    /// Run this after changing the embedding model or provider: it lets
    /// the store keep its memories instead of needing a fresh data
    /// directory. Stop the daemon first — it rebuilds the vector index.
    Reindex {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Merge duplicate memories now, instead of waiting for the timer.
    Consolidate {
        /// Report what would be merged and change nothing. Calls no
        /// model, so it costs nothing to run.
        #[arg(long)]
        dry_run: bool,
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
        Command::Config { config } => run_config(config.as_deref()),
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
        Command::Reindex { config } => run_reindex(config.as_deref()),
        Command::Consolidate { dry_run, config } => {
            run_consolidate(dry_run, config.as_deref()).await
        }
        Command::User { command, config } => run_user(command, config.as_deref()),
        Command::Key { command, config } => run_key(command, config.as_deref()),
    };

    if let Err(message) = result {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

/// Every context, wired.
///
/// Both `serve` and `consolidate` need the whole graph — the nightly job
/// touches identity (to walk users), memories (to read and write them)
/// and understanding (for the model) — so building it lives in one place
/// rather than being assembled twice and drifting.
struct Wired {
    identity: bootstrap::wiring::Identity,
    memories: bootstrap::memories_wiring::Memories,
    understanding: bootstrap::understanding_wiring::Understanding,
    consolidation: bootstrap::consolidation_wiring::Consolidation,
}

fn build_all(config: &bootstrap::config::AppConfig) -> Result<Wired, String> {
    // Every context shares one database handle: SQLite has a single
    // writer, and two handles would mean two connections contending for
    // the same file rather than queueing behind one mutex.
    let database = bootstrap::wiring::open_database(config).map_err(|e| e.to_string())?;
    let shared_database = std::sync::Arc::clone(&database);
    let identity = bootstrap::wiring::Identity::from_database(std::sync::Arc::clone(&database))
        .map_err(|e| e.to_string())?;
    let memories =
        bootstrap::memories_wiring::Memories::build(config, std::sync::Arc::clone(&database))
            .map_err(|e| e.to_string())?;
    let understanding =
        bootstrap::understanding_wiring::Understanding::build(config, database, &memories)
            .map_err(|e| e.to_string())?;
    let consolidation = bootstrap::consolidation_wiring::Consolidation::build(
        config,
        &identity,
        &memories,
        &understanding,
        shared_database,
    )
    .map_err(|e| e.to_string())?;

    Ok(Wired {
        identity,
        memories,
        understanding,
        consolidation,
    })
}

async fn run_consolidate(dry_run: bool, config_path: Option<&Path>) -> Result<(), String> {
    bootstrap::server::init_tracing();

    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;
    let wired = build_all(&config)?;

    // Runs with or without a provider: expiry and decay need no model,
    // and only merging is skipped without one.
    consolidation::infrastructure::cli::run(wired.consolidation.runner.clone(), dry_run)
        .await
        .map_err(|e| e.to_string())
}

async fn run_serve(config_path: Option<&Path>) -> Result<(), String> {
    bootstrap::server::init_tracing();

    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;
    let Wired {
        identity,
        memories,
        understanding,
        consolidation,
    } = build_all(&config)?;

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

    let consolidation = std::sync::Arc::new(consolidation);

    // The nightly job. `None` when `[consolidation].enabled` is false.
    let scheduler = consolidation::infrastructure::consolidation_scheduler::start(
        std::sync::Arc::clone(&consolidation.runner),
        consolidation.enabled,
        &consolidation.schedule,
    )
    .map_err(|e| e.to_string())?;

    let state = bootstrap::state::AppState {
        identity,
        memories: std::sync::Arc::new(memories),
        understanding,
        consolidation,
        auth_mode: bootstrap::state::AuthMode::from_config(&config),
        mcp_http: config.server.mcp.http,
    };

    let outcome = bootstrap::server::serve(&config.server.host, config.server.port, state).await;

    // After the listener drains, so a job enqueued by the last in-flight
    // request still gets picked up rather than waiting for a restart.
    workers.shutdown().await;
    if let Some(scheduler) = scheduler {
        scheduler.shutdown().await;
    }

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
        .embed(
            &["warm".to_string()],
            memories::domain::embedder::EmbeddingTask::Document,
        )
        .map_err(|e| e.to_string())?;

    println!("model ready ({} dimensions)", embedder.dimensions());
    Ok(())
}

fn run_reindex(config_path: Option<&Path>) -> Result<(), String> {
    bootstrap::server::init_tracing();

    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;

    // Build the *new* embedder first — a misconfigured provider should
    // stop here, before a single vector is touched. For a remote provider
    // this also probes it end to end.
    let embedder =
        bootstrap::memories_wiring::build_embedder(&config).map_err(|e| e.to_string())?;
    let database = bootstrap::wiring::open_database(&config).map_err(|e| e.to_string())?;

    println!(
        "re-embedding every memory with {} ({} dimensions)…",
        embedder.model_id(),
        embedder.dimensions()
    );

    let report =
        memories::infrastructure::sqlite_reindexer::SqliteReindexer::new(database, embedder)
            .execute()
            .map_err(|e| e.to_string())?;

    match report.from {
        Some((model, dims)) if (model.as_str(), dims) != (report.to.0.as_str(), report.to.1) => {
            println!(
                "reindexed {} memories: {model} ({dims}) → {} ({})",
                report.reindexed, report.to.0, report.to.1
            );
        }
        _ => println!(
            "reindexed {} memories under {} ({})",
            report.reindexed, report.to.0, report.to.1
        ),
    }
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

fn run_config(config_path: Option<&Path>) -> Result<(), String> {
    // No tracing init: this is a one-shot report, and its whole value is
    // clean stdout the operator can read at a glance.
    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;
    let embeddings = &config.embeddings;
    let understanding = &config.understanding;

    println!("recuerdos-ai configuration (defaults + file + RECUERDOS_AI_* env)\n");

    println!("embeddings");
    println!("  provider   {}", embeddings.provider);
    println!("  model      {}", embeddings.model);
    if embeddings.provider == "local" {
        // The one field that only matters offline; a remote provider
        // ignores it, so showing it there would just mislead.
        println!("  cache_dir  {}", config.model_cache_dir().display());
    } else {
        println!("  base_url   {}", display_base_url(&embeddings.base_url));
        println!("  api_key    {}", key_status(&embeddings.api_key_env));
    }

    println!("\nunderstanding");
    println!("  provider   {}", understanding.provider);
    if understanding.provider != "none" {
        println!("  model      {}", understanding.model);
        println!("  base_url   {}", display_base_url(&understanding.base_url));
        println!("  api_key    {}", key_status(&understanding.api_key_env));
        println!("  reconcile  {}", understanding.reconcile);
    }

    println!("\nconsolidation");
    if config.consolidation.enabled {
        println!("  enabled ({})", config.consolidation.schedule);
    } else {
        println!("  disabled");
    }

    println!("\nserver");
    println!("  listen     {}:{}", config.server.host, config.server.port);
    let mut transports = Vec::new();
    if config.server.mcp.http {
        transports.push("http (/mcp)");
    }
    if config.server.mcp.stdio {
        transports.push("stdio (recuerdos-ai mcp)");
    }
    println!(
        "  mcp        {}",
        if transports.is_empty() {
            "none".to_string()
        } else {
            transports.join(", ")
        }
    );

    println!("\nstorage");
    println!("  backend    {}", config.storage.backend);
    println!("  path       {}", config.data_dir().display());

    println!("\nauth");
    println!("  mode       {}", config.auth.mode);

    Ok(())
}

/// How a `base_url` reads in the report: an empty one means the provider's
/// built-in address, which is clearer said than shown blank.
fn display_base_url(base_url: &str) -> String {
    if base_url.trim().is_empty() {
        "(provider default)".to_string()
    } else {
        base_url.to_string()
    }
}

/// Report an API key by the env var that holds it and whether that var is
/// currently set — never the value. An empty name means the provider needs
/// no key (a local server); a named-but-unset var is the likely cause of a
/// provider that fails on its first real request, so it is called out.
fn key_status(env_name: &str) -> String {
    if env_name.trim().is_empty() {
        "none".to_string()
    } else if std::env::var(env_name).is_ok() {
        format!("{env_name} (set)")
    } else {
        format!("{env_name} (NOT SET)")
    }
}

fn run_init(config_path: &PathBuf) -> Result<(), String> {
    const EXAMPLE_CONFIG: &str = include_str!("../recuerdos-ai.example.toml");

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
