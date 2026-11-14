//! What the daemon says about itself: its version, where its configuration
//! came from, and the settings in effect.
//!
//! One report, two readers: `recuerdos-ai config` prints it, and
//! `GET /v1/about` (the web UI's About page) serves it. It never holds a
//! secret: an API key appears as the name of the env var that holds it and
//! whether that var is set, and an override appears by name only.

use crate::bootstrap::config::AppConfig;
use crate::bootstrap::memories_wiring::resolve_similarity_baseline;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The env var that points the whole CLI at a config file.
const CONFIG_ENV: &str = "RECUERDOS_AI_CONFIG";

/// Where the configuration file was named, if anywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigOrigin {
    /// `serve --config <path>`.
    Flag,
    /// `RECUERDOS_AI_CONFIG`.
    Env,
    /// Neither: built-in defaults and `RECUERDOS_AI_*` overrides only.
    None,
}

/// The configuration file the daemon was started with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigFile {
    pub path: Option<PathBuf>,
    pub from: ConfigOrigin,
    /// A named file that does not exist is not an error (defaults apply), so
    /// it is said here rather than discovered by surprise.
    pub exists: bool,
}

/// One section of the report: `embeddings`, `retrieval`, …
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Section {
    pub name: &'static str,
    pub settings: Vec<Setting>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Setting {
    pub key: &'static str,
    pub value: String,
}

/// Everything the About page shows to an operator.
#[derive(Debug, Clone, Serialize)]
pub struct About {
    pub version: &'static str,
    pub git_sha: &'static str,
    pub started_at: DateTime<Utc>,
    /// The command line the daemon was started with (`serve --config …`).
    pub command: Vec<String>,
    pub config_file: ConfigFile,
    /// The `RECUERDOS_AI_*` variables set when it started, by name.
    pub overrides: Vec<String>,
    pub settings: Vec<Section>,
}

impl About {
    /// Captured once at start: what the daemon is running with, not what the
    /// environment says now.
    pub fn capture(config: &AppConfig, flag: Option<&Path>, now: DateTime<Utc>) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            git_sha: env!("RECUERDOS_AI_GIT_SHA"),
            started_at: now,
            command: std::env::args().skip(1).collect(),
            config_file: config_file(flag, std::env::var(CONFIG_ENV).ok().as_deref()),
            overrides: overrides(std::env::vars().map(|(name, _)| name)),
            settings: report(config),
        }
    }
}

/// Which file `AppConfig::load` reads, by the same precedence: `--config`,
/// then a non-blank `RECUERDOS_AI_CONFIG`.
pub fn config_file(flag: Option<&Path>, env: Option<&str>) -> ConfigFile {
    let env = env
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from);
    let (path, from) = match (flag, env) {
        (Some(path), _) => (Some(path.to_path_buf()), ConfigOrigin::Flag),
        (None, Some(path)) => (Some(path), ConfigOrigin::Env),
        (None, None) => (None, ConfigOrigin::None),
    };
    let exists = path.as_deref().is_some_and(Path::is_file);
    ConfigFile { path, from, exists }
}

/// The names of the `RECUERDOS_AI_*` variables among `names`, sorted. Values
/// are left out on purpose: some name secrets.
pub fn overrides(names: impl Iterator<Item = String>) -> Vec<String> {
    let mut found: Vec<String> = names
        .filter(|name| name.starts_with("RECUERDOS_AI_"))
        .collect();
    found.sort();
    found.dedup();
    found
}

/// The settings in effect, after defaults, the file and the env are merged.
pub fn report(config: &AppConfig) -> Vec<Section> {
    let mut sections = Vec::new();

    let embeddings = &config.embeddings;
    let mut rows = vec![
        row("provider", &embeddings.provider),
        row("model", &embeddings.model),
    ];
    if embeddings.provider == "local" {
        // The one field that only matters offline; a remote provider ignores
        // it, so showing it there would just mislead.
        rows.push(row("cache_dir", config.model_cache_dir().display()));
    } else {
        rows.push(row("base_url", display_base_url(&embeddings.base_url)));
        rows.push(row("api_key", key_status(&embeddings.api_key_env)));
    }
    // In every state: which text the vectors are built from also decides
    // which store will open.
    rows.push(row("document_text", &embeddings.document_text));
    sections.push(section("embeddings", rows));

    let understanding = &config.understanding;
    let mut rows = vec![row("provider", &understanding.provider)];
    if understanding.provider != "none" {
        rows.push(row("model", &understanding.model));
        rows.push(row("base_url", display_base_url(&understanding.base_url)));
        rows.push(row("api_key", key_status(&understanding.api_key_env)));
        rows.push(row("reconcile", understanding.reconcile));
    }
    sections.push(section("understanding", rows));

    let consolidation = if config.consolidation.enabled {
        format!("enabled ({})", config.consolidation.schedule)
    } else {
        "disabled".to_string()
    };
    sections.push(section("consolidation", vec![row("state", consolidation)]));

    // Retrieval decides what comes back and in what order; zero weights print
    // too, because "the order is what fusion decided" is an effective state.
    let retrieval = &config.retrieval;
    sections.push(section(
        "retrieval",
        vec![
            row("limit", retrieval.default_limit),
            row(
                "recency",
                format!("{} d half-life", retrieval.recency_half_life_days),
            ),
            row("floor", retrieval.min_relevance),
            row(
                "similarity_baseline",
                format!(
                    "{} ({})",
                    resolve_similarity_baseline(config, &embeddings.model),
                    if retrieval.similarity_baseline.is_some() {
                        "explicit"
                    } else {
                        "per-model default"
                    }
                ),
            ),
            row("relevance_weight", retrieval.relevance_weight),
        ],
    ));

    // In every mode: `off` is an effective state as much as `on` is.
    let rerank = &config.rerank;
    let mut rows = vec![row("mode", &rerank.mode)];
    if rerank.mode != "off" {
        rows.push(row("model", &rerank.model));
        rows.push(row("depth", rerank.depth));
        rows.push(row("weight", rerank.weight));
        rows.push(row("timeout", format!("{} ms", rerank.timeout_ms)));
        rows.push(row("concurrent", rerank.max_concurrency));
        rows.push(row("batch", rerank.batch_size));
    }
    sections.push(section("rerank", rows));

    let graph = &config.graph;
    let mut rows = vec![row("enabled", graph.enabled)];
    if graph.enabled {
        rows.push(row(
            "hops",
            format!("{} (limit {})", graph.max_hops, graph.hop_limit),
        ));
        rows.push(row(
            "weight",
            format!(
                "{} (unanchored floor {})",
                graph.rank_weight, graph.unanchored_floor
            ),
        ));
        rows.push(row("expansion_slots", graph.expansion_slots));
    }
    sections.push(section("graph", rows));

    let mut transports = Vec::new();
    if config.server.mcp.http {
        transports.push("http (/mcp)");
    }
    if config.server.mcp.stdio {
        transports.push("stdio (recuerdos-ai mcp)");
    }
    sections.push(section(
        "server",
        vec![
            row(
                "listen",
                format!("{}:{}", config.server.host, config.server.port),
            ),
            row(
                "mcp",
                if transports.is_empty() {
                    "none".to_string()
                } else {
                    transports.join(", ")
                },
            ),
            row("ui", if config.ui.enabled { "/ui" } else { "off" }),
        ],
    ));

    sections.push(section(
        "storage",
        vec![
            row("backend", &config.storage.backend),
            row("path", config.data_dir().display()),
        ],
    ));

    sections.push(section("auth", vec![row("mode", &config.auth.mode)]));

    sections
}

fn section(name: &'static str, settings: Vec<Setting>) -> Section {
    Section { name, settings }
}

fn row(key: &'static str, value: impl std::fmt::Display) -> Setting {
    Setting {
        key,
        value: value.to_string(),
    }
}

/// An empty `base_url` means the provider's built-in address, which is
/// clearer said than shown blank.
fn display_base_url(base_url: &str) -> String {
    if base_url.trim().is_empty() {
        "(provider default)".to_string()
    } else {
        base_url.to_string()
    }
}

/// An API key by the env var that holds it and whether that var is set —
/// never the value. A named-but-unset var is the likely cause of a provider
/// that fails on its first real request, so it is called out.
fn key_status(env_name: &str) -> String {
    if env_name.trim().is_empty() {
        "none".to_string()
    } else if std::env::var(env_name).is_ok() {
        format!("{env_name} (set)")
    } else {
        format!("{env_name} (NOT SET)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_wins_over_the_env_var_as_load_does() {
        let file = config_file(Some(Path::new("/etc/ra.toml")), Some("/other.toml"));
        assert_eq!(file.path.as_deref(), Some(Path::new("/etc/ra.toml")));
        assert_eq!(file.from, ConfigOrigin::Flag);
        assert!(!file.exists, "no such file here");

        let file = config_file(None, Some("/other.toml"));
        assert_eq!(file.from, ConfigOrigin::Env);

        let file = config_file(None, Some("  "));
        assert_eq!(file.from, ConfigOrigin::None, "a blank var names nothing");
        assert_eq!(file.path, None);
    }

    #[test]
    fn overrides_are_names_only_sorted_and_ours() {
        let names = [
            "RECUERDOS_AI_SERVER__PORT",
            "HOME",
            "RECUERDOS_AI_AUTH__MODE",
            "OPENAI_API_KEY",
        ]
        .map(String::from);
        assert_eq!(
            overrides(names.into_iter()),
            vec!["RECUERDOS_AI_AUTH__MODE", "RECUERDOS_AI_SERVER__PORT"]
        );
    }

    #[test]
    fn the_report_names_a_key_by_its_variable_never_its_value() {
        let mut config = AppConfig::default();
        config.understanding.provider = "openai-compat".to_string();
        config.understanding.api_key_env = "RA_TEST_SURELY_UNSET_KEY".to_string();

        let sections = report(&config);

        let understanding = sections.iter().find(|s| s.name == "understanding").unwrap();
        let key = understanding
            .settings
            .iter()
            .find(|s| s.key == "api_key")
            .unwrap();
        assert_eq!(key.value, "RA_TEST_SURELY_UNSET_KEY (NOT SET)");
        let names: Vec<_> = sections.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            [
                "embeddings",
                "understanding",
                "consolidation",
                "retrieval",
                "rerank",
                "graph",
                "server",
                "storage",
                "auth"
            ]
        );
    }
}
