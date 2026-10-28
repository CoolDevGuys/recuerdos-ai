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
    /// meaningless without the real one. `--config` decides *which*
    /// embedder (and floor, and tuning) is measured: pass the production
    /// config to measure production.
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
        /// Run the Phase 7 tuning grid instead of one scored pass
        /// (reranking-plan.md task 7.2): one seeded corpus, every
        /// model × strategy × depth × weight row, the table printed as
        /// cells finish with Δs against the `off` reference. Incompatible
        /// with the baseline flags — a grid is a measurement, not a gate.
        #[arg(long)]
        grid: bool,
        /// Comma list of rerank models for the grid. Defaults to
        /// `[rerank].model` — the grid's tuned winner — because a bare
        /// `eval --grid` should measure what ships. The large multilingual
        /// models need gigabytes of weights and RAM, so the operator names
        /// the set they can afford. Every name `[rerank].model` accepts
        /// works.
        #[arg(long, default_value = "bge-reranker-base")]
        grid_models: String,
        /// Comma list of head depths for the grid (the plan's table).
        #[arg(long, default_value = "10,20,30,50")]
        grid_depths: String,
        /// Comma list of RRF weights for the grid; `replace` rows have no
        /// weight and are measured once per depth.
        #[arg(long, default_value = "0.5,1,2,4")]
        grid_weights: String,
        /// Comma list of fusion strategies for the grid: `rrf`, `replace`.
        #[arg(long, default_value = "rrf,replace")]
        grid_strategies: String,
        /// The machine-readable grid artifact instead of the table: every
        /// cell with its per-kind p@1 and both latency pairs, safe to
        /// commit or re-gate offline. Grid mode only.
        #[arg(long)]
        json: bool,
    },
    /// Download the local embedding model (and the rerank model, when
    /// `[rerank].mode` is not `off`) into the cache directory.
    ///
    /// Run at image build time so a container never downloads at
    /// runtime, or before taking a host offline.
    WarmModels {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Report the relevance calibration of the configured embedder.
    ///
    /// Hidden, like `eval`: an operator/dev measurement, not a service.
    /// Embeds the eval set's corpus and cases with the embedder in `--config`
    /// and reports the two numbers the relevance floor has to sit inside —
    /// the **noise ceiling** (max cosine of the `negative` queries against
    /// the corpus) and the **signal floor** (min cosine of each case's
    /// expected memory). Read them to set `[retrieval].similarity_baseline`
    /// when adding an embedding model; the map in
    /// `memories/domain/recall_ranker.rs` is populated from runs like this
    /// (reranking-plan.md Phase 2.1).
    #[command(hide = true)]
    Calibrate {
        #[arg(long, default_value = "eval/cases.toml")]
        cases: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Compare recall with and without the reranker, over memories that
    /// already exist.
    ///
    /// Hidden, like `eval` and `calibrate`: an operator measuring a decision,
    /// not something a client calls. Replays every query in `--queries` twice
    /// over the store `--config` points at — once through the pipeline as it
    /// serves today, once with the second stage switched on — and prints both
    /// orderings, what moved, and the cost.
    ///
    /// It changes nothing: the keyword index is opened read-only and both
    /// recalls are untracked, so memories, access counters and importance are
    /// byte-identical afterwards. The daemon may stay running.
    ///
    /// This is also the only supported way to run the reranker over an
    /// existing corpus: there is no `rerank all` command, deliberately —
    /// rerank scores are a property of a candidate list, not of a stored
    /// memory (reranking-plan.md §4).
    #[command(hide = true)]
    Rerank {
        #[command(subcommand)]
        command: RerankCommand,
        #[arg(long, global = true)]
        config: Option<PathBuf>,
    },
    /// Compare two recall configurations over memories that already exist.
    ///
    /// Hidden, like `eval` and `rerank audit`: an operator measuring a decision,
    /// not something a client calls. Replays every query in `--queries` twice
    /// over the store `--config` points at — once through that config's
    /// pipeline, once through `--compare-config`'s — and prints both orderings,
    /// what moved, and what each side cost.
    ///
    /// With no `--compare-config` the two columns are the same config replayed
    /// twice. That is not a no-op: it is the self-comparison, and it must show
    /// zero changes. It is how a store's current ranking gets recorded before
    /// anything is allowed to change it.
    ///
    /// It changes nothing: the keyword index is opened read-only and both
    /// recalls are untracked, so memories, access counters and importance are
    /// byte-identical afterwards. The daemon may stay running.
    #[command(hide = true)]
    Recall {
        #[command(subcommand)]
        command: RecallCommand,
        /// The config the `before` column runs — and the one whose `[storage]`
        /// holds the store being audited.
        #[arg(long, global = true)]
        config: Option<PathBuf>,
        /// The config the `after` column runs. Its `[storage]` and
        /// `[embeddings]` must match `--config`: the vectors in a store were
        /// built by exactly one model, so a second column measured against a
        /// different one would describe nothing. Refused rather than reported.
        #[arg(long, global = true)]
        compare_config: Option<PathBuf>,
    },
    /// Rebuild derived indexes from the `memories` table.
    ///
    /// With no flags this is the vector reindex: re-embed every memory
    /// under the current [embeddings] model, in place. Run it after
    /// changing the embedding model or provider; it lets the store keep
    /// its memories instead of needing a fresh data directory.
    ///
    /// `--text` rebuilds the keyword (BM25) index instead — no model
    /// calls, seconds at personal scale; `--vectors` names the vector
    /// pass explicitly, and both flags run both. Pass any flag at all
    /// and the daemon must be *stopped* first (the keyword rebuild
    /// needs the tantivy writer to itself, the vector pass holds the
    /// SQLite write lock).
    Reindex {
        /// Rebuild the keyword (BM25) index for every user.
        #[arg(long)]
        text: bool,
        /// Re-embed the vector index (the default when no flag is given).
        #[arg(long)]
        vectors: bool,
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
    /// Build the entity/relation graph for an existing corpus.
    ///
    /// Populates the graph tables for memories that predate it (or were
    /// ingested with `[graph].enabled = false`), so turning the graph on
    /// does not leave old memories unreachable by a hop. Runs regardless of
    /// `[graph].enabled` — backfill first, then enable.
    Graph {
        #[command(subcommand)]
        command: GraphCommand,
        #[arg(long, global = true)]
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

#[derive(Subcommand)]
enum RecallCommand {
    /// Replay queries over one user's real memories under two configs and
    /// report the two orderings side by side.
    ///
    /// Writes nothing — no access counters, no index writes — so the daemon may
    /// stay running. A side whose `[rerank].mode` is `on` or `shadow` loads that
    /// side's model; a side with `off` loads nothing, so comparing two stage-less
    /// configs costs no gigabyte of weights.
    Audit {
        /// Query file: one query per line (`#` comments allowed), or TOML
        /// `[[query]]` blocks with `text` and optional `expect` — a substring of
        /// the memory that *should* come first.
        #[arg(long)]
        queries: PathBuf,
        /// Whose memories to audit. Required when the store has more than one
        /// user; never guessed.
        #[arg(long)]
        user: Option<String>,
        /// Results per query to print and score. Also the `k` of complete@k.
        #[arg(long, default_value_t = 8)]
        limit: usize,
        /// Force `[rerank].depth` on both sides instead of each side's own.
        /// Set it to compare two configs at one depth; leave it unset to compare
        /// what each config would actually run.
        #[arg(long)]
        depth: Option<usize>,
        /// The machine-readable report instead of the table: ids, ranks and
        /// scores, no memory content, safe to commit or paste.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RerankCommand {
    /// Replay queries over one user's real memories and report the two
    /// orderings: the pipeline as it serves today, and with the reranker on.
    ///
    /// Writes nothing — no access counters, no index writes — so the daemon
    /// may stay running. Loading the rerank model is the point, so it happens
    /// whatever `[rerank].mode` says.
    Audit {
        /// Query file: one query per line (`#` comments allowed), or TOML
        /// `[[query]]` blocks with `text` and optional `expect` — a substring
        /// of the memory that *should* come first.
        #[arg(long)]
        queries: PathBuf,
        /// Whose memories to audit. Required when the store has more than one
        /// user; never guessed.
        #[arg(long)]
        user: Option<String>,
        /// Results per query to print and score.
        #[arg(long, default_value_t = 8)]
        limit: usize,
        /// How many fused candidates the reranker judges. Defaults to
        /// `[rerank].depth`, which is what a live request would use.
        #[arg(long)]
        depth: Option<usize>,
        /// The machine-readable report instead of the table: ids, ranks and
        /// scores, no memory content, safe to commit or paste.
        #[arg(long)]
        json: bool,
    },
    /// Report the cross-encoder's score distribution over the eval corpus:
    /// expected pairs versus noise (reranking-plan.md task 7.4).
    ///
    /// Scores every case query against every corpus memory directly — no
    /// indexes, no fusion, no floor — so the separability verdict the
    /// relevance decision reads is measured against the worst the corpus
    /// can do, not the head fusion happened to offer. Loads
    /// `[rerank].model` whatever `[rerank].mode` says, like the audit.
    Distribution {
        #[arg(long, default_value = "eval/cases.toml")]
        cases: PathBuf,
        /// The machine-readable report instead of the table. Carries scores
        /// only — no memory content.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum GraphCommand {
    /// Backfill the graph from what memories already carry.
    Backfill {
        /// Project every memory's stored entities into the graph. Makes no
        /// model call, so it is free and safe to re-run.
        #[arg(long)]
        entities: bool,
        /// Re-extract relations for memories that have none. Costs one model
        /// call each, bounded by [graph].backfill_budget, and resumes where
        /// a previous run stopped.
        #[arg(long)]
        relations: bool,
        /// Report what each pass would do — memories, and the model calls
        /// relations would make — and change nothing.
        #[arg(long)]
        dry_run: bool,
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
            grid,
            grid_models,
            grid_depths,
            grid_weights,
            grid_strategies,
            json,
        } => run_eval(
            &cases,
            baseline.as_deref(),
            max_drop,
            write_baseline.as_deref(),
            config.as_deref(),
            GridInvocation {
                grid,
                models: &grid_models,
                depths: &grid_depths,
                weights: &grid_weights,
                strategies: &grid_strategies,
                json,
            },
        ),
        Command::WarmModels { config } => run_warm_models(config.as_deref()),
        Command::Calibrate { cases, config } => run_calibrate(&cases, config.as_deref()),
        Command::Rerank { command, config } => run_rerank(command, config.as_deref()),
        Command::Recall {
            command,
            config,
            compare_config,
        } => run_recall(command, config.as_deref(), compare_config.as_deref()),
        Command::Reindex {
            text,
            vectors,
            config,
        } => run_reindex(text, vectors, config.as_deref()),
        Command::Consolidate { dry_run, config } => {
            run_consolidate(dry_run, config.as_deref()).await
        }
        Command::Graph { command, config } => run_graph(command, config.as_deref()).await,
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

async fn run_graph(command: GraphCommand, config_path: Option<&Path>) -> Result<(), String> {
    match command {
        GraphCommand::Backfill {
            entities,
            relations,
            dry_run,
        } => run_graph_backfill(entities, relations, dry_run, config_path).await,
    }
}

async fn run_graph_backfill(
    entities: bool,
    relations: bool,
    dry_run: bool,
    config_path: Option<&Path>,
) -> Result<(), String> {
    if !entities && !relations {
        return Err("nothing to do: pass --entities, --relations, or both".to_string());
    }

    bootstrap::server::init_tracing();
    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;
    let database = bootstrap::wiring::open_database(&config).map_err(|e| e.to_string())?;
    let identity = bootstrap::wiring::Identity::from_database(std::sync::Arc::clone(&database))
        .map_err(|e| e.to_string())?;
    let memories =
        bootstrap::memories_wiring::Memories::build(&config, std::sync::Arc::clone(&database))
            .map_err(|e| e.to_string())?;
    let understanding = bootstrap::understanding_wiring::Understanding::build(
        &config,
        std::sync::Arc::clone(&database),
        &memories,
    )
    .map_err(|e| e.to_string())?;

    // The graph and its watermark are built directly on the database, not
    // taken from `memories.graph` — that is `None` until `[graph].enabled`,
    // and the whole point of backfill is to populate the graph *before*
    // turning it on.
    let graph: std::sync::Arc<dyn memories::domain::entity_graph::EntityGraph> =
        std::sync::Arc::new(
            memories::infrastructure::sqlite_entity_graph::SqliteEntityGraph::new(
                std::sync::Arc::clone(&database),
            ),
        );
    let state: std::sync::Arc<dyn memories::domain::graph_backfill_state::GraphBackfillState> =
        std::sync::Arc::new(
            memories::infrastructure::sqlite_graph_backfill_state::SqliteGraphBackfillState::new(
                database,
            ),
        );

    // Relation backfill wants edges regardless of the runtime `extract_relations`
    // flag — an operator running it has decided to spend on relations.
    let extractor = understanding.model.clone().map(|model| {
        std::sync::Arc::new(
            understanding::application::candidate_extractor::CandidateExtractor::new(
                model,
                std::sync::Arc::clone(&understanding.taxonomy),
                true,
            ),
        )
    });

    let budget_config = &config.graph.backfill_budget;
    let budget = consolidation::application::consolidation_runner::BudgetLimits {
        max_llm_calls: (budget_config.max_llm_calls != 0).then_some(budget_config.max_llm_calls),
        max_duration_secs: (budget_config.max_duration_secs != 0)
            .then_some(budget_config.max_duration_secs),
        max_memories: (budget_config.max_memories != 0).then_some(budget_config.max_memories),
    };

    let backfiller = consolidation::application::graph_backfiller::GraphBackfiller::new(
        std::sync::Arc::clone(&identity.users),
        std::sync::Arc::clone(&memories.repository),
        graph,
        state,
        extractor,
        budget,
    );

    if entities {
        let report = backfiller
            .backfill_entities(dry_run)
            .map_err(|e| e.to_string())?;
        println!("{}", render_backfill(&report));
    }
    if relations {
        let report = backfiller
            .backfill_relations(dry_run)
            .await
            .map_err(|e| e.to_string())?;
        println!("{}", render_backfill(&report));
    }
    Ok(())
}

fn render_backfill(
    report: &consolidation::application::graph_backfiller::BackfillReport,
) -> String {
    use consolidation::application::graph_backfiller::BackfillPass;

    let mut out = match report.pass {
        BackfillPass::Entities => {
            let verb = if report.dry_run {
                "would project"
            } else {
                "projected"
            };
            format!(
                "entities: {verb} {} of {} memories examined across {} users (no model calls)",
                report.memories_projected, report.memories_examined, report.users
            )
        }
        BackfillPass::Relations if report.dry_run => format!(
            "relations: {} memories examined across {} users; \
             {} model calls would be made",
            report.memories_examined, report.users, report.llm_calls
        ),
        BackfillPass::Relations => format!(
            "relations: {} model calls over {} memories examined across {} users; \
             {} gained edges",
            report.llm_calls, report.memories_examined, report.users, report.relations_added
        ),
    };

    if report.budget_exhausted {
        out.push_str(&format!(
            "\nstopped early: {} — re-run to continue where it left off",
            report.budget_reason.as_deref().unwrap_or("budget reached")
        ));
    }
    if report.dry_run {
        out.push_str("\nRe-run without --dry-run to apply.");
    }
    out
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

    // The keyword index is versioned (reranking-plan.md Phase 1.4): a
    // store from before an index-version bump has memories but no index
    // directory, and serving it as-is would mean a silently dead keyword
    // leg. Rebuild those users first — no model calls, seconds at
    // personal scale, and a no-op check (one stat per user) every other
    // start. A failure here fails the start rather than serving a
    // crippled recall; the message says what to do about it.
    let rebuilder = memories::application::text_index_rebuilder::TextIndexRebuilder::new(
        std::sync::Arc::clone(&identity.users),
        std::sync::Arc::clone(&memories.repository),
        std::sync::Arc::clone(&memories.text_index),
    );
    let rebuild = rebuilder.rebuild_missing().map_err(|e| {
        format!(
            "the keyword index could not be rebuilt: {e}; \
             stop the daemon and run `recuerdos-ai reindex --text` \
             with the same config"
        )
    })?;
    if rebuild.users_rebuilt > 0 {
        println!(
            "rebuilt the keyword index for {} users ({} memories)",
            rebuild.users_rebuilt, rebuild.memories_indexed
        );
    }

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
        // Refresh the cached profile digest after an ingest changes a
        // user's memories, so `GET /v1/profile` never has to generate it
        // on the read path. The writer is the only subscriber.
        observer: Some(std::sync::Arc::clone(&consolidation.profile_digest_writer)
            as std::sync::Arc<
                dyn memories::domain::memory_change_observer::MemoryChangeObserver,
            >),
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
        rate_limit: config.rate_limit.clone(),
        mcp_http: config.server.mcp.http,
        mcp_allowed_hosts: config.server.mcp.allowed_hosts.clone(),
        ingest_timeout: std::time::Duration::from_secs(config.server.ingest_timeout_secs),
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

/// The grid half of the `eval` flags, bundled so the scored path never has
/// to look at them: `grid == false` means they do not exist.
struct GridInvocation<'a> {
    grid: bool,
    models: &'a str,
    depths: &'a str,
    weights: &'a str,
    strategies: &'a str,
    json: bool,
}

fn run_eval(
    cases: &Path,
    baseline: Option<&Path>,
    max_drop: f64,
    write_baseline: Option<&Path>,
    config_path: Option<&Path>,
    grid: GridInvocation<'_>,
) -> Result<(), String> {
    // The eval runs against the config's embedder, floor and ranker
    // tuning (reranking-plan.md Phase 0.1): production runs
    // `gemini-embedding-2`, and scores measured on a different embedder
    // do not transfer. The store itself stays a throwaway the eval seeds
    // and discards, so the config's `[storage].path` is never touched.
    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;

    if grid.grid {
        // A grid is a measurement, not a gate: its reference is the `off`
        // row inside it, so there is no baseline to compare against and no
        // single run to record.
        if baseline.is_some() || write_baseline.is_some() {
            return Err(
                "--grid cannot be combined with --baseline/--write-baseline — gate the scored \
                 path with those; the grid carries its own off reference"
                    .to_string(),
            );
        }
        let options = bootstrap::rerank_grid::GridOptions::parse(
            grid.models,
            grid.strategies,
            grid.depths,
            grid.weights,
        )
        .map_err(|e| e.to_string())?;
        let report = bootstrap::rerank_grid::run(cases, &config, &options, grid.json)
            .map_err(|e| e.to_string())?;
        if grid.json {
            let encoded = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
            println!("{encoded}");
        }
        return Ok(());
    }
    if grid.json {
        return Err(
            "--json reports the grid; a scored eval prints its report and gates on --baseline"
                .to_string(),
        );
    }

    // `run` yields the gated report — informational cases, which document
    // known defects and fail on purpose, are excluded from both writing
    // and comparison so a baseline only ever records the cases the gate
    // runs (task 0.5).
    let report = bootstrap::eval::run(cases, &config).map_err(|e| e.to_string())?;

    if let Some(path) = write_baseline {
        return bootstrap::eval::write_baseline(&report, path).map_err(|e| e.to_string());
    }
    if let Some(path) = baseline {
        return bootstrap::eval::compare(&report, path, max_drop).map_err(|e| e.to_string());
    }
    Ok(())
}

/// `rerank` subcommands: the report, on stdout, and nothing else on stdout.
///
/// No tracing initialisation, unlike `consolidate` or `reindex` — the
/// audit's progress line goes to stderr precisely so `--json > run.json`
/// stays parseable, and a logging layer would put it back on the wrong
/// stream. The distribution subcommand keeps the same discipline.
fn run_rerank(command: RerankCommand, config_path: Option<&Path>) -> Result<(), String> {
    // Same config semantics as `eval` and `calibrate`: `--config` decides
    // which embedder, which rerank model and which tuning is being measured,
    // so judging production means passing production's config.
    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;
    match command {
        RerankCommand::Audit {
            queries,
            user,
            limit,
            depth,
            json,
        } => {
            print_audit(&config, None, &queries, user.as_deref(), limit, depth, json)?;
        }
        RerankCommand::Distribution { cases, json } => {
            // The human table is printed by the module; `--json` prints
            // only the artifact, like the audit.
            let report = bootstrap::rerank_distribution::run(&cases, &config, json)
                .map_err(|e| e.to_string())?;
            if json {
                let encoded = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
                println!("{encoded}");
            }
        }
    }
    Ok(())
}

/// `recall` subcommands: the report on stdout, and nothing else on stdout
/// (same stream discipline as [`run_rerank`]).
///
/// Both columns always come from a loaded config here, `--compare-config`
/// defaulting to `--config` itself: `recall audit` with no second file is a
/// self-comparison, which is the measurement that must come back with zero
/// changes (recall-quality-plan.md Task 0.9). The forced off/on pair belongs to
/// `rerank audit`, which passes `compare: None` and gets the alias's behaviour.
fn run_recall(
    command: RecallCommand,
    config_path: Option<&Path>,
    compare_path: Option<&Path>,
) -> Result<(), String> {
    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;
    // No second file means the same config on both sides — the same *loaded*
    // value, not a second read of the file, so the two columns cannot disagree
    // about anything, including what an unset key defaults to.
    let compare = match compare_path {
        Some(path) => {
            Some(bootstrap::config::AppConfig::load(Some(path)).map_err(|e| e.to_string())?)
        }
        None => None,
    };
    let compare = compare.as_ref().unwrap_or(&config);

    match command {
        RecallCommand::Audit {
            queries,
            user,
            limit,
            depth,
            json,
        } => print_audit(
            &config,
            Some(compare),
            &queries,
            user.as_deref(),
            limit,
            depth,
            json,
        ),
    }
}

/// Runs one audit and renders it.
///
/// Both commands come through here: the alias and the two-config comparison are
/// the same measurement over different sides, and a second copy of the
/// rendering — or of the refusal to compare two stores — would drift, and the
/// drift would be read as a finding.
fn print_audit(
    config: &bootstrap::config::AppConfig,
    compare: Option<&bootstrap::config::AppConfig>,
    queries: &Path,
    user: Option<&str>,
    limit: usize,
    depth: Option<usize>,
    json: bool,
) -> Result<(), String> {
    let report = bootstrap::rerank_audit::run(
        config,
        &bootstrap::rerank_audit::AuditOptions {
            queries,
            user,
            limit,
            depth,
            compare,
        },
    )
    .map_err(|e| e.to_string())?;

    if json {
        let encoded = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        println!("{encoded}");
    } else {
        bootstrap::rerank_audit::print_human(&report);
    }
    Ok(())
}

fn run_calibrate(cases: &Path, config_path: Option<&Path>) -> Result<(), String> {
    // Same config semantics as `eval`: `--config` decides *which* embedder
    // is measured, so production's model (and its `[retrieval]` numbers)
    // are what the report describes (reranking-plan.md Phase 2.1).
    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;
    bootstrap::calibrate::run(cases, &config).map_err(|e| e.to_string())
}

fn run_warm_models(config_path: Option<&Path>) -> Result<(), String> {
    use memories::domain::{embedder::Embedder, reranker::Reranker};

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
            cache_dir.clone(),
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

    // The rerank model joins the bake-in only when the feature is
    // enabled in the config being warmed (reranking-plan.md Phase 5.4):
    // the default config has no rerank model, and the release image
    // should not carry one nobody asked for. The same proof applies as
    // above — score a pair so a truncated download fails the build, not
    // the first recall. One session is enough to prove the files; the
    // daemon loads its own `max_concurrency` pool at boot.
    if config.rerank.mode != "off" {
        println!(
            "\ndownloading rerank model {} (mode {}) into {}",
            config.rerank.model,
            config.rerank.mode,
            cache_dir.display()
        );
        // The phase's documented measurements (reranking-plan.md §12):
        // cold-start is the load itself; the RSS delta around it is what
        // one session's weights add to a serving process at idle. Linux
        // reports this from procfs; elsewhere the line says so plainly.
        let before_rss = process_memory_kib();
        let started = std::time::Instant::now();
        let reranker =
            providers::infrastructure::rerank::fastembed_reranker::FastembedReranker::load(
                &config.rerank.model,
                cache_dir,
                1,
                config.rerank.batch_size,
            )
            .map_err(|e| e.to_string())?;
        let load_ms = started.elapsed().as_secs_f64() * 1000.0;
        let candidates = [
            memories::domain::reranker::RerankCandidate {
                id: shared::ids::MemoryId::new(),
                text: "warm",
            },
            memories::domain::reranker::RerankCandidate {
                id: shared::ids::MemoryId::new(),
                text: "cold",
            },
        ];
        let started = std::time::Instant::now();
        let scores = reranker
            .score("warm", &candidates)
            .map_err(|e| e.to_string())?;
        let score_ms = started.elapsed().as_secs_f64() * 1000.0;
        let after_rss = process_memory_kib();
        println!(
            "rerank model ready ({} candidates scored in {:.1} ms)",
            scores.len(),
            score_ms
        );
        println!(
            "measurements: load {:.0} ms · RSS {} -> {} KiB ({})",
            load_ms,
            before_rss.unwrap_or(0),
            after_rss.unwrap_or(0),
            match (before_rss, after_rss) {
                (Some(b), Some(a)) if a > b => format!("+{} KiB", a - b),
                _ => "delta unavailable".to_string(),
            }
        );
    }

    Ok(())
}

/// Resident set of this process in KiB, where the OS reports it.
fn process_memory_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Both indexes are derived from `memories`, but built by different
/// machinery: vectors need the embedder (and the model), the keyword
/// index needs nothing but the repository. Neither flag is the *old*
/// command — vectors only — so scripts written before `--text` existed
/// keep doing exactly what they did.
fn run_reindex(text: bool, vectors: bool, config_path: Option<&Path>) -> Result<(), String> {
    bootstrap::server::init_tracing();

    // No flag at all keeps today's behaviour: vectors only.
    let do_vectors = vectors || !text;

    let config = bootstrap::config::AppConfig::load(config_path).map_err(|e| e.to_string())?;

    // Build the *new* embedder first — a misconfigured provider should
    // stop here, before a single vector is touched. For a remote provider
    // this also probes it end to end.
    let embedder =
        bootstrap::memories_wiring::build_embedder(&config).map_err(|e| e.to_string())?;
    let database = bootstrap::wiring::open_database(&config).map_err(|e| e.to_string())?;

    if do_vectors {
        println!(
            "re-embedding every memory with {} ({} dimensions)…",
            embedder.model_id(),
            embedder.dimensions()
        );

        let report = memories::infrastructure::sqlite_reindexer::SqliteReindexer::new(
            std::sync::Arc::clone(&database),
            std::sync::Arc::clone(&embedder),
        )
        .with_document_text(config.document_text())
        .execute()
        .map_err(|e| e.to_string())?;

        match report.from {
            Some((model, dims))
                if (model.as_str(), dims) != (report.to.0.as_str(), report.to.1) =>
            {
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
    }

    if text {
        // The keyword index lives under the same data directory and is
        // rebuilt from the same table — no model calls, so this half of
        // the command works with the embedding provider unreachable.
        // (The instance still needs the configured embedder to *open*
        // the store; that is the model pin, not the rebuild.)
        let identity = bootstrap::wiring::Identity::from_database(std::sync::Arc::clone(&database))
            .map_err(|e| e.to_string())?;
        let wired = bootstrap::memories_wiring::Memories::assemble(
            &config,
            std::sync::Arc::clone(&database),
            std::sync::Arc::clone(&embedder),
        )
        .map_err(|e| e.to_string())?;

        let rebuilder = memories::application::text_index_rebuilder::TextIndexRebuilder::new(
            std::sync::Arc::clone(&identity.users),
            std::sync::Arc::clone(&wired.repository),
            std::sync::Arc::clone(&wired.text_index),
        );

        println!("rebuilding the keyword index for every user…");
        let report = rebuilder.rebuild_all().map_err(|e| e.to_string())?;

        for user in &report.rebuilt {
            println!("  {}: {} memories", user.handle, user.memories_indexed);
        }
        println!(
            "rebuilt the keyword index for {} users ({} memories, {} users skipped)",
            report.users_rebuilt, report.memories_indexed, report.users_skipped
        );
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
    // Both print in every state: which text the vectors are built from is
    // the one embedding setting that also decides which store will open, so
    // "content" is an answer an operator checking a deploy needs to see.
    println!("  document_text {}", embeddings.document_text);
    if embeddings.query_instruction.trim().is_empty() {
        println!("  query_instruction none");
    } else {
        println!("  query_instruction {:?}", embeddings.query_instruction);
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

    // Retrieval decides what comes back and in what order, so it is worth
    // confirming without opening the file. `relevance_weight` prints in every
    // state including its zero, for the same reason the rerank mode does: "the
    // order is what fusion decided" is an effective state, and 0.0 is the answer
    // an operator needs to hear rather than a line to hide.
    let retrieval = &config.retrieval;
    println!("\nretrieval");
    println!("  hybrid     {}", retrieval.hybrid);
    println!("  limit      {}", retrieval.default_limit);
    println!(
        "  recency    {} d half-life",
        retrieval.recency_half_life_days
    );
    println!("  floor      {}", retrieval.min_relevance);
    // The baseline the ranker will actually use — explicit, or the map row
    // for this model *and* document format — because an explicit value
    // silently outranks the enriched row (docs/configuration.md).
    println!(
        "  similarity_baseline {} ({})",
        bootstrap::memories_wiring::resolve_similarity_baseline(&config, &embeddings.model),
        if retrieval.similarity_baseline.is_some() {
            "explicit"
        } else {
            "per-model default"
        }
    );
    println!("  relevance_weight {}", retrieval.relevance_weight);

    // The rerank stage prints in every mode: `off` is its effective
    // state as much as `on` is, and the whole point of this report is
    // answering "what is the daemon actually doing" without opening the
    // config file (reranking-plan.md Phase 5.2).
    let rerank = &config.rerank;
    println!("\nrerank");
    println!("  mode       {}", rerank.mode);
    if rerank.mode != "off" {
        println!("  model      {}", rerank.model);
        // The model lands in the embeddings cache dir — shared by
        // convention, so showing it here would duplicate that section.
        println!("  depth      {}", rerank.depth);
        println!(
            "  strategy   {} (weight {})",
            rerank.strategy, rerank.weight
        );
        println!("  timeout    {} ms", rerank.timeout_ms);
        println!("  concurrent {}", rerank.max_concurrency);
        println!("  batch      {}", rerank.batch_size);
    }

    // The third leg decides which relational memories are reachable at all, and
    // since Phase 3 it can also reserve a seat in the answer — so its state belongs
    // here for the same reason retrieval's and the rerank stage's do: a deployment
    // where the hop places nothing and one where it holds a slot are different
    // systems, and neither is visible without opening the file.
    let graph = &config.graph;
    println!("\ngraph");
    println!("  enabled    {}", graph.enabled);
    if graph.enabled {
        println!(
            "  hops       {} (limit {})",
            graph.max_hops, graph.hop_limit
        );
        println!(
            "  weight     {} (unanchored floor {})",
            graph.rank_weight, graph.unanchored_floor
        );
        // Printed at zero too: `0` means the hop competes for rank and nothing
        // else, which is the pre-Phase-3 pipeline and an effective state an
        // operator comparing two deployments needs to see.
        println!("  expansion_slots {}", graph.expansion_slots);
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
