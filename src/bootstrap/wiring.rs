//! Composition root for the identity context.
//!
//! The one place that sees both the traits and their concrete
//! implementations. Every other module receives what it needs already
//! wired, which is what keeps the layer rules in `docs/architecture.md`
//! enforceable rather than aspirational.

use crate::bootstrap::config::AppConfig;
use crate::identity::application::api_key_issuer::ApiKeyIssuer;
use crate::identity::application::api_key_lister::ApiKeyLister;
use crate::identity::application::api_key_revoker::ApiKeyRevoker;
use crate::identity::application::default_user_resolver::DefaultUserResolver;
use crate::identity::application::key_authenticator::KeyAuthenticator;
use crate::identity::application::user_creator::UserCreator;
use crate::identity::domain::api_key_hasher::ApiKeyHasher;
use crate::identity::domain::api_key_repository::ApiKeyRepository;
use crate::identity::domain::user_repository::UserRepository;
use crate::identity::infrastructure::argon2_api_key_hasher::Argon2ApiKeyHasher;
use crate::identity::infrastructure::sqlite_api_key_repository::SqliteApiKeyRepository;
use crate::identity::infrastructure::sqlite_user_repository::SqliteUserRepository;
use crate::shared::clock::{Clock, SystemClock};
use crate::shared::error::Result;
use crate::shared::sqlite::SqliteDatabase;
use std::sync::Arc;

/// The database file inside the configured data directory.
pub const DATABASE_FILE: &str = "recordagent.db";

pub struct Identity {
    pub user_creator: Arc<UserCreator>,
    pub api_key_issuer: Arc<ApiKeyIssuer>,
    pub api_key_revoker: Arc<ApiKeyRevoker>,
    pub api_key_lister: Arc<ApiKeyLister>,
    pub key_authenticator: Arc<KeyAuthenticator>,
    pub default_user_resolver: Arc<DefaultUserResolver>,
    /// Exposed for the `auth.mode = "none"` bootstrap user and for
    /// `user list`; callers outside this module should prefer a use case.
    pub users: Arc<dyn UserRepository>,
    pub keys: Arc<dyn ApiKeyRepository>,
    pub clock: Arc<dyn Clock>,
}

impl Identity {
    pub fn build(config: &AppConfig) -> Result<Self> {
        let database = Arc::new(SqliteDatabase::open(
            &config.data_dir().join(DATABASE_FILE),
        )?);
        Self::from_database(database)
    }

    /// Wires against an already-open database. Used by tests to build the
    /// same object graph over an in-memory store.
    pub fn from_database(database: Arc<SqliteDatabase>) -> Result<Self> {
        let users: Arc<dyn UserRepository> =
            Arc::new(SqliteUserRepository::new(Arc::clone(&database)));
        let keys: Arc<dyn ApiKeyRepository> =
            Arc::new(SqliteApiKeyRepository::new(Arc::clone(&database)));
        let hasher: Arc<dyn ApiKeyHasher> = Arc::new(Argon2ApiKeyHasher::new());
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);

        Ok(Self {
            user_creator: Arc::new(UserCreator::new(Arc::clone(&users), Arc::clone(&clock))),
            api_key_issuer: Arc::new(ApiKeyIssuer::new(
                Arc::clone(&users),
                Arc::clone(&keys),
                Arc::clone(&hasher),
                Arc::clone(&clock),
            )),
            api_key_revoker: Arc::new(ApiKeyRevoker::new(Arc::clone(&keys), Arc::clone(&clock))),
            api_key_lister: Arc::new(ApiKeyLister::new(Arc::clone(&users), Arc::clone(&keys))),
            key_authenticator: Arc::new(KeyAuthenticator::new(
                Arc::clone(&users),
                Arc::clone(&keys),
                Arc::clone(&hasher),
            )),
            default_user_resolver: Arc::new(DefaultUserResolver::new(
                Arc::clone(&users),
                Arc::clone(&clock),
            )),
            users,
            keys,
            clock,
        })
    }
}
