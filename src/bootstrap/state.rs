//! Shared axum state: what every handler and extractor can reach.

use crate::bootstrap::config::AppConfig;
use crate::bootstrap::memories_wiring::Memories;
use crate::bootstrap::understanding_wiring::Understanding;
use crate::bootstrap::wiring::Identity;
use std::sync::Arc;

/// Whether requests must present a credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// `[auth].mode = "api-key"` — the default.
    ApiKey,
    /// `[auth].mode = "none"` — every request runs as the built-in
    /// `default` user. For a single-user deployment on a trusted host.
    None,
}

impl AuthMode {
    /// Config validation has already restricted this to a known value, so
    /// anything unexpected here means the two lists drifted apart —
    /// fail closed (require a key) rather than silently open the server.
    pub fn from_config(config: &AppConfig) -> Self {
        match config.auth.mode.as_str() {
            "none" => AuthMode::None,
            _ => AuthMode::ApiKey,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub identity: Arc<Identity>,
    pub memories: Arc<Memories>,
    pub understanding: Arc<Understanding>,
    pub auth_mode: AuthMode,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_mode(mode: &str) -> AppConfig {
        AppConfig {
            auth: crate::bootstrap::config::AuthConfig {
                mode: mode.to_string(),
            },
            ..AppConfig::default()
        }
    }

    #[test]
    fn reads_the_configured_mode() {
        assert_eq!(
            AuthMode::from_config(&config_with_mode("api-key")),
            AuthMode::ApiKey
        );
        assert_eq!(
            AuthMode::from_config(&config_with_mode("none")),
            AuthMode::None
        );
    }

    #[test]
    fn an_unrecognised_mode_fails_closed() {
        assert_eq!(
            AuthMode::from_config(&config_with_mode("anything-else")),
            AuthMode::ApiKey,
            "an unknown mode must require a key, never disable auth"
        );
    }
}
