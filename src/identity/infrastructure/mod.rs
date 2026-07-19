//! Adapters: SQLite repositories, the auth middleware/extractor, CLI
//! `user`/`key` subcommands — everything that touches the outside world.

pub mod argon2_api_key_hasher;
pub mod sqlite_api_key_repository;
pub mod sqlite_user_repository;
