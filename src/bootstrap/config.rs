//! `AppConfig`: typed config loaded from `recuerdos-ai.toml`, overridable via
//! `RECUERDOS_AI_*` env vars (nested keys via `__`, e.g.
//! `RECUERDOS_AI_SERVER__PORT=8080`). Precedence: defaults → file → env.
//!
//! Lives in `bootstrap` (not a business context) because loading and
//! validating config is a composition-root concern per
//! implementation-plan.md §0.3.

use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// All config errors found in one pass, so a user fixes every problem in
/// one edit-run cycle instead of playing whack-a-mole.
#[derive(Debug, thiserror::Error)]
#[error("config error: {}", .0.join("; "))]
pub struct ConfigError(pub Vec<String>);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub embeddings: EmbeddingsConfig,
    pub understanding: UnderstandingConfig,
    pub consolidation: ConsolidationConfig,
    pub retrieval: RetrievalConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub mcp: McpConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 7070,
            mcp: McpConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    pub stdio: bool,
    pub http: bool,
    /// Extra `Host` header values the streamable-HTTP `/mcp` endpoint will
    /// accept, on top of the loopback defaults (`localhost`, `127.0.0.1`,
    /// `::1`) that the MCP spec's DNS-rebinding guard allows.
    ///
    /// Empty (the default) keeps the guard loopback-only, which is right
    /// when a reverse proxy fronts the daemon. Set it to the hostname(s)
    /// clients actually connect with — e.g. `["memory.example.com"]` or
    /// `["memory.example.com:7070"]` — to allow direct, proxy-less network
    /// access. A single `"*"` entry disables the guard entirely (accept any
    /// `Host`); only sensible on a trusted private network.
    pub allowed_hosts: Vec<String>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            stdio: true,
            http: true,
            allowed_hosts: Vec::new(),
        }
    }
}

const STORAGE_BACKENDS: &[&str] = &["embedded", "postgres", "qdrant"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub backend: String,
    pub path: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: "embedded".to_string(),
            path: "~/.recuerdos-ai/data".to_string(),
        }
    }
}

const EMBEDDINGS_PROVIDERS: &[&str] = &["local", "gemini", "openai-compat", "ollama"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingsConfig {
    pub provider: String,
    pub model: String,
    /// Where the local model files live. Set explicitly rather than left
    /// to the library's default, which is the process's working
    /// directory — for a daemon, "wherever it happened to be started
    /// from". The Docker image points this at a baked-in `/models`.
    /// Ignored by the remote providers.
    pub cache_dir: String,
    /// Name of the env var holding the provider's API key, mirroring
    /// `[understanding]`. Empty means "no key" — correct for a local
    /// server (Ollama, a self-hosted vLLM) and rejected by a hosted one.
    /// The key itself never lives in the config file.
    pub api_key_env: String,
    /// Override the provider's address, for an OpenAI-compatible gateway
    /// or a non-default Ollama host. Empty means the provider's usual
    /// address. Ignored by the local provider.
    pub base_url: String,
}

impl Default for EmbeddingsConfig {
    fn default() -> Self {
        Self {
            // Local by default: embeddings are the one thing that must
            // work with no account, no key and no network, or the "runs
            // fully offline" claim is false.
            provider: "local".to_string(),
            model: "bge-small-en-v1.5".to_string(),
            cache_dir: "~/.recuerdos-ai/models".to_string(),
            api_key_env: String::new(),
            base_url: String::new(),
        }
    }
}

const UNDERSTANDING_PROVIDERS: &[&str] =
    &["anthropic", "openai-compat", "gemini", "ollama", "none"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UnderstandingConfig {
    pub provider: String,
    pub model: String,
    pub api_key_env: String,
    /// Where to reach the provider. Empty means "the provider's usual
    /// address", which is what almost everyone wants; it exists for
    /// OpenAI-compatible gateways (OpenRouter, Groq, a local vLLM) and
    /// for a non-default Ollama host — and it is what the contract tests
    /// point at a mock server.
    pub base_url: String,
    pub reconcile: bool,
    /// How many ingest jobs may run concurrently.
    ///
    /// Two by default: enough that one slow model call does not stall the
    /// queue behind it, low enough that a burst of submissions cannot
    /// fan out into a rate limit or a surprise bill.
    pub workers: usize,
    /// Attempts before an ingest job dead-letters.
    pub max_attempts: u32,
    pub taxonomy: TaxonomyConfig,
}

impl Default for UnderstandingConfig {
    fn default() -> Self {
        Self {
            // Zero-egress by default (project-plan.md §7.2): understanding
            // is opt-in, embeddings (above) work fully offline already.
            provider: "none".to_string(),
            model: "claude-haiku-4-5".to_string(),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            base_url: String::new(),
            reconcile: true,
            workers: 2,
            max_attempts: 3,
            taxonomy: TaxonomyConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaxonomyConfig {
    pub extra_categories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConsolidationConfig {
    pub enabled: bool,
    pub schedule: String,
    pub similarity_threshold: f64,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            schedule: "daily".to_string(),
            similarity_threshold: 0.92,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetrievalConfig {
    pub hybrid: bool,
    pub default_limit: u32,
    pub recency_half_life_days: u32,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            hybrid: true,
            default_limit: 8,
            recency_half_life_days: 90,
        }
    }
}

const AUTH_MODES: &[&str] = &["api-key", "none"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    pub mode: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: "api-key".to_string(),
        }
    }
}

impl AppConfig {
    /// Load config from (in ascending precedence) built-in defaults, an
    /// optional TOML file, then `RECUERDOS_AI_*` env vars. Returns every
    /// validation problem found, not just the first.
    pub fn load(config_path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut figment = Figment::from(Serialized::defaults(AppConfig::default()));

        // An explicit `--config` wins; otherwise fall back to the
        // `RECUERDOS_AI_CONFIG` env var. That fallback is what lets one
        // setting point the *whole* CLI at a file — `serve`, `reindex`,
        // `consolidate`, `config` — without repeating `--config` on each
        // command (the Docker workflow sets it once for every invocation).
        // A missing file is not an error: `Toml::file` merges nothing and
        // you fall back to defaults + env, same as with no config at all.
        //
        // `RECUERDOS_AI_CONFIG` is read here, not through the `Env` provider
        // below, so it never looks like a config key — figment ignores it
        // there just as it ignores any other unknown `RECUERDOS_AI_*` var.
        let env_config = std::env::var("RECUERDOS_AI_CONFIG")
            .ok()
            .filter(|value| !value.trim().is_empty());
        if let Some(path) = config_path.or(env_config.as_deref().map(Path::new)) {
            figment = figment.merge(Toml::file(path));
        }

        figment = figment.merge(Env::prefixed("RECUERDOS_AI_").split("__"));

        let config: AppConfig = figment
            .extract()
            .map_err(|e| ConfigError(vec![format!("failed to parse config: {e}")]))?;

        let issues = config.validate();
        if !issues.is_empty() {
            return Err(ConfigError(issues));
        }

        Ok(config)
    }

    /// All validation problems in the config, empty if none. Each message
    /// names the TOML path it complains about, e.g. `[server].port is 0`.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        if self.server.host.trim().is_empty() {
            issues.push("[server].host is empty".to_string());
        }
        if self.server.port == 0 {
            issues.push("[server].port is 0".to_string());
        }

        if !STORAGE_BACKENDS.contains(&self.storage.backend.as_str()) {
            issues.push(format!(
                "[storage].backend {:?} is not one of {STORAGE_BACKENDS:?}",
                self.storage.backend
            ));
        }
        if self.storage.path.trim().is_empty() {
            issues.push("[storage].path is empty".to_string());
        }

        if !EMBEDDINGS_PROVIDERS.contains(&self.embeddings.provider.as_str()) {
            issues.push(format!(
                "[embeddings].provider {:?} is not one of {EMBEDDINGS_PROVIDERS:?}",
                self.embeddings.provider
            ));
        }
        if self.embeddings.model.trim().is_empty() {
            issues.push("[embeddings].model is empty".to_string());
        }
        // Only the local provider reads cache_dir, so only it must have
        // one — a remote provider legitimately leaves it blank.
        if self.embeddings.provider == "local" && self.embeddings.cache_dir.trim().is_empty() {
            issues.push("[embeddings].cache_dir is empty".to_string());
        }

        if !UNDERSTANDING_PROVIDERS.contains(&self.understanding.provider.as_str()) {
            issues.push(format!(
                "[understanding].provider {:?} is not one of {UNDERSTANDING_PROVIDERS:?}",
                self.understanding.provider
            ));
        }
        if self.understanding.provider != "none" && self.understanding.model.trim().is_empty() {
            issues.push("[understanding].model is empty".to_string());
        }
        if self.understanding.workers == 0 {
            issues.push("[understanding].workers is 0 — no ingest job would ever run".to_string());
        }
        if self.understanding.max_attempts == 0 {
            issues.push(
                "[understanding].max_attempts is 0 — every job would dead-letter unattempted"
                    .to_string(),
            );
        }

        // Checked here rather than only when the scheduler starts: an
        // unrecognised schedule would otherwise mean consolidation
        // silently never runs, which looks exactly like it running and
        // finding nothing.
        if let Err(error) =
            crate::consolidation::infrastructure::consolidation_scheduler::interval_for(
                &self.consolidation.schedule,
            )
        {
            issues.push(error.to_string().replace("validation failed: ", ""));
        }
        if !(0.0..=1.0).contains(&self.consolidation.similarity_threshold) {
            issues.push(format!(
                "[consolidation].similarity_threshold {} is out of range 0.0..=1.0",
                self.consolidation.similarity_threshold
            ));
        }

        if self.retrieval.default_limit == 0 {
            issues.push("[retrieval].default_limit is 0".to_string());
        }
        if self.retrieval.recency_half_life_days == 0 {
            issues.push("[retrieval].recency_half_life_days is 0".to_string());
        }

        if !AUTH_MODES.contains(&self.auth.mode.as_str()) {
            issues.push(format!(
                "[auth].mode {:?} is not one of {AUTH_MODES:?}",
                self.auth.mode
            ));
        }

        issues
    }

    /// Resolve `storage.path`, expanding a leading `~` to `$HOME`.
    pub fn data_dir(&self) -> PathBuf {
        expand_home(&self.storage.path)
    }

    /// Resolve `embeddings.cache_dir`, expanding a leading `~`.
    pub fn model_cache_dir(&self) -> PathBuf {
        expand_home(&self.embeddings.cache_dir)
    }
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
// Jail::expect_with closures return figment's Result<(), figment::Error>;
// figment::Error is intentionally rich (path + reason) and this is test-only
// code, never a hot path — the perf lint doesn't apply here.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;
    use figment::Jail;

    // Every test that touches env vars or files runs inside a `Jail`:
    // figment's own test harness, which sandboxes env var mutation and cwd
    // so parallel `cargo test` threads don't race each other. It restores
    // both on drop regardless of ambient ones set by whoever runs the
    // suite (docker-compose deliberately keeps RECUERDOS_AI_* out of the
    // `dev` service's container-wide environment for this reason).

    #[test]
    fn defaults_only_load_and_validate() {
        Jail::expect_with(|_jail| {
            let config = AppConfig::load(None).expect("defaults must be valid");
            assert_eq!(config.server.port, 7070);
            assert_eq!(config.storage.backend, "embedded");
            assert_eq!(config.understanding.provider, "none");
            Ok(())
        });
    }

    #[test]
    fn file_overrides_defaults() {
        Jail::expect_with(|jail| {
            jail.create_file("recuerdos-ai.toml", "[server]\nport = 9999\n")?;

            let config = AppConfig::load(Some(Path::new("recuerdos-ai.toml"))).unwrap();
            assert_eq!(config.server.port, 9999);
            // Untouched fields keep their defaults.
            assert_eq!(config.server.host, "127.0.0.1");
            Ok(())
        });
    }

    #[test]
    fn recuerdos_ai_config_env_names_the_file_when_no_flag_is_passed() {
        Jail::expect_with(|jail| {
            jail.create_file("elsewhere.toml", "[server]\nport = 4242\n")?;
            // No `--config` argument; the env var alone points at the file.
            jail.set_env("RECUERDOS_AI_CONFIG", "elsewhere.toml");

            let config = AppConfig::load(None).unwrap();
            assert_eq!(config.server.port, 4242);
            Ok(())
        });
    }

    #[test]
    fn an_explicit_config_flag_wins_over_the_env_var() {
        Jail::expect_with(|jail| {
            jail.create_file("flag.toml", "[server]\nport = 1111\n")?;
            jail.create_file("env.toml", "[server]\nport = 2222\n")?;
            jail.set_env("RECUERDOS_AI_CONFIG", "env.toml");

            let config = AppConfig::load(Some(Path::new("flag.toml"))).unwrap();
            assert_eq!(config.server.port, 1111, "the explicit flag must win");
            Ok(())
        });
    }

    #[test]
    fn env_overrides_file() {
        Jail::expect_with(|jail| {
            jail.create_file("recuerdos-ai.toml", "[server]\nport = 9999\n")?;
            jail.set_env("RECUERDOS_AI_SERVER__PORT", "1234");

            let config = AppConfig::load(Some(Path::new("recuerdos-ai.toml"))).unwrap();
            assert_eq!(config.server.port, 1234);
            Ok(())
        });
    }

    #[test]
    fn invalid_values_are_all_reported_together() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "recuerdos-ai.toml",
                "[server]\nport = 0\n[embeddings]\nmodel = \"\"\n",
            )?;

            let err = AppConfig::load(Some(Path::new("recuerdos-ai.toml"))).unwrap_err();
            assert!(err.0.iter().any(|m| m.contains("[server].port is 0")));
            assert!(
                err.0
                    .iter()
                    .any(|m| m.contains("[embeddings].model is empty"))
            );
            assert_eq!(
                err.0.len(),
                2,
                "expected exactly these two issues: {:?}",
                err.0
            );
            Ok(())
        });
    }

    #[test]
    fn a_remote_embeddings_provider_needs_no_cache_dir() {
        // cache_dir is a local-only concern; a remote provider leaving it
        // blank must not be a config error.
        Jail::expect_with(|jail| {
            // The dev container exports RECUERDOS_AI_EMBEDDINGS__CACHE_DIR,
            // and env wins over the file — null it so the empty cache_dir
            // under test actually reaches validation.
            jail.set_env("RECUERDOS_AI_EMBEDDINGS__CACHE_DIR", "");
            jail.create_file(
                "recuerdos-ai.toml",
                "[embeddings]\nprovider = \"openai-compat\"\n\
                 model = \"text-embedding-3-small\"\n",
            )?;

            let config = AppConfig::load(Some(Path::new("recuerdos-ai.toml")))
                .expect("a remote provider with no cache_dir should load");
            assert_eq!(config.embeddings.provider, "openai-compat");
            assert!(config.embeddings.cache_dir.is_empty());
            Ok(())
        });
    }

    #[test]
    fn the_local_provider_still_requires_a_cache_dir() {
        Jail::expect_with(|jail| {
            jail.set_env("RECUERDOS_AI_EMBEDDINGS__CACHE_DIR", "");
            jail.create_file("recuerdos-ai.toml", "[embeddings]\nprovider = \"local\"\n")?;

            let err = AppConfig::load(Some(Path::new("recuerdos-ai.toml"))).unwrap_err();
            assert!(
                err.0.iter().any(|m| m.contains("[embeddings].cache_dir")),
                "{:?}",
                err.0
            );
            Ok(())
        });
    }

    #[test]
    fn an_unknown_embeddings_provider_is_rejected() {
        Jail::expect_with(|jail| {
            jail.create_file("recuerdos-ai.toml", "[embeddings]\nprovider = \"cohere\"\n")?;

            let err = AppConfig::load(Some(Path::new("recuerdos-ai.toml"))).unwrap_err();
            assert!(err.0.iter().any(|m| m.contains("[embeddings].provider")));
            Ok(())
        });
    }

    #[test]
    fn unknown_storage_backend_is_rejected() {
        Jail::expect_with(|jail| {
            jail.create_file("recuerdos-ai.toml", "[storage]\nbackend = \"dynamodb\"\n")?;

            let err = AppConfig::load(Some(Path::new("recuerdos-ai.toml"))).unwrap_err();
            assert!(err.0.iter().any(|m| m.contains("[storage].backend")));
            Ok(())
        });
    }

    #[test]
    fn home_expansion() {
        Jail::expect_with(|jail| {
            jail.set_env("HOME", "/home/tester");
            let config = AppConfig {
                storage: StorageConfig {
                    path: "~/.recuerdos-ai/data".to_string(),
                    ..StorageConfig::default()
                },
                ..AppConfig::default()
            };
            assert_eq!(
                config.data_dir(),
                PathBuf::from("/home/tester/.recuerdos-ai/data")
            );
            Ok(())
        });
    }
}
