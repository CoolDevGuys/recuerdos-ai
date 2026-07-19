//! `AppConfig`: typed config loaded from `recordagent.toml`, overridable via
//! `RECORDAGENT_*` env vars (nested keys via `__`, e.g.
//! `RECORDAGENT_SERVER__PORT=8080`). Precedence: defaults → file → env.
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
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            stdio: true,
            http: true,
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
            path: "~/.recordagent/data".to_string(),
        }
    }
}

const EMBEDDINGS_PROVIDERS: &[&str] = &["local", "openai-compat", "ollama"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingsConfig {
    pub provider: String,
    pub model: String,
    /// Where the local model files live. Set explicitly rather than left
    /// to the library's default, which is the process's working
    /// directory — for a daemon, "wherever it happened to be started
    /// from". The Docker image points this at a baked-in `/models`.
    pub cache_dir: String,
}

impl Default for EmbeddingsConfig {
    fn default() -> Self {
        Self {
            provider: "local".to_string(),
            model: "bge-small-en-v1.5".to_string(),
            cache_dir: "~/.recordagent/models".to_string(),
        }
    }
}

const UNDERSTANDING_PROVIDERS: &[&str] = &["anthropic", "openai-compat", "ollama", "none"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UnderstandingConfig {
    pub provider: String,
    pub model: String,
    pub api_key_env: String,
    pub reconcile: bool,
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
            reconcile: true,
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
    /// optional TOML file, then `RECORDAGENT_*` env vars. Returns every
    /// validation problem found, not just the first.
    pub fn load(config_path: Option<&Path>) -> Result<Self, ConfigError> {
        let mut figment = Figment::from(Serialized::defaults(AppConfig::default()));

        if let Some(path) = config_path {
            figment = figment.merge(Toml::file(path));
        }

        figment = figment.merge(Env::prefixed("RECORDAGENT_").split("__"));

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
        if self.embeddings.cache_dir.trim().is_empty() {
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

        if self.consolidation.schedule.trim().is_empty() {
            issues.push("[consolidation].schedule is empty".to_string());
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
    // suite (docker-compose deliberately keeps RECORDAGENT_* out of the
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
            jail.create_file("recordagent.toml", "[server]\nport = 9999\n")?;

            let config = AppConfig::load(Some(Path::new("recordagent.toml"))).unwrap();
            assert_eq!(config.server.port, 9999);
            // Untouched fields keep their defaults.
            assert_eq!(config.server.host, "127.0.0.1");
            Ok(())
        });
    }

    #[test]
    fn env_overrides_file() {
        Jail::expect_with(|jail| {
            jail.create_file("recordagent.toml", "[server]\nport = 9999\n")?;
            jail.set_env("RECORDAGENT_SERVER__PORT", "1234");

            let config = AppConfig::load(Some(Path::new("recordagent.toml"))).unwrap();
            assert_eq!(config.server.port, 1234);
            Ok(())
        });
    }

    #[test]
    fn invalid_values_are_all_reported_together() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "recordagent.toml",
                "[server]\nport = 0\n[embeddings]\nmodel = \"\"\n",
            )?;

            let err = AppConfig::load(Some(Path::new("recordagent.toml"))).unwrap_err();
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
    fn unknown_storage_backend_is_rejected() {
        Jail::expect_with(|jail| {
            jail.create_file("recordagent.toml", "[storage]\nbackend = \"dynamodb\"\n")?;

            let err = AppConfig::load(Some(Path::new("recordagent.toml"))).unwrap_err();
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
                    path: "~/.recordagent/data".to_string(),
                    ..StorageConfig::default()
                },
                ..AppConfig::default()
            };
            assert_eq!(
                config.data_dir(),
                PathBuf::from("/home/tester/.recordagent/data")
            );
            Ok(())
        });
    }
}
