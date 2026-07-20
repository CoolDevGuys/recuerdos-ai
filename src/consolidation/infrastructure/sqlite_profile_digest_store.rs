//! `ProfileDigestStore` backed by SQLite.
//!
//! Every statement carries `user_id = ?` from an authenticated
//! [`UserContext`], for the same reason the memory repository does: this
//! table holds a summary of one person's entire memory store, which
//! makes a missing scope here about as costly as a missing scope gets.

use crate::consolidation::domain::profile_digest::{Domain, ProfileDigestStore, StoredDigest};
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::{RaError, Result};
use crate::shared::sqlite::{SqliteDatabase, map_sqlite_error, optional};
use chrono::{DateTime, Utc};
use std::sync::Arc;

pub struct SqliteProfileDigestStore {
    database: Arc<SqliteDatabase>,
}

impl SqliteProfileDigestStore {
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }
}

impl ProfileDigestStore for SqliteProfileDigestStore {
    fn find(&self, context: &UserContext, domain: Domain) -> Result<Option<StoredDigest>> {
        self.database.with_connection(|connection| {
            optional(connection.query_row(
                "SELECT content, fingerprint, generated_at FROM profile_digests
                 WHERE user_id = ?1 AND domain = ?2",
                rusqlite::params![context.user_id().to_string(), domain.as_str()],
                |row| {
                    Ok(build_digest(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            ))
        })
    }

    fn save(&self, context: &UserContext, domain: Domain, digest: &StoredDigest) -> Result<()> {
        self.database.with_connection(|connection| {
            // Upsert: a digest has exactly one current value per domain,
            // and previous generations are of no interest — unlike a
            // memory, nothing here was ever asserted by the user.
            connection
                .execute(
                    "INSERT INTO profile_digests
                        (user_id, domain, content, fingerprint, generated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (user_id, domain) DO UPDATE SET
                        content = excluded.content,
                        fingerprint = excluded.fingerprint,
                        generated_at = excluded.generated_at",
                    rusqlite::params![
                        context.user_id().to_string(),
                        domain.as_str(),
                        digest.content,
                        digest.fingerprint,
                        digest.generated_at.to_rfc3339(),
                    ],
                )
                .map_err(|e| map_sqlite_error(e, "digest write conflict"))?;
            Ok(())
        })
    }
}

fn build_digest(
    content: String,
    fingerprint: String,
    generated_at: String,
) -> Result<StoredDigest> {
    Ok(StoredDigest {
        content,
        fingerprint,
        generated_at: DateTime::parse_from_rfc3339(&generated_at)
            .map(|value| value.with_timezone(&Utc))
            .map_err(|e| {
                RaError::Internal(format!("stored digest timestamp {generated_at:?}: {e}"))
            })?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::domain::scope::Scope;

    struct Fixture {
        store: SqliteProfileDigestStore,
        alex: UserContext,
        sam: UserContext,
    }

    fn fixture() -> Fixture {
        let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
        let identity =
            crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();

        Fixture {
            store: SqliteProfileDigestStore::new(database),
            alex: authenticate(&identity, "alex"),
            sam: authenticate(&identity, "sam"),
        }
    }

    /// The only way anything mints a context — by authenticating.
    fn authenticate(identity: &crate::bootstrap::wiring::Identity, handle: &str) -> UserContext {
        identity.user_creator.execute(handle, None).unwrap();
        let issued = identity
            .api_key_issuer
            .execute(handle, vec![Scope::Admin], "test")
            .unwrap();
        identity
            .key_authenticator
            .execute(&issued.token.render())
            .unwrap()
    }

    fn digest(content: &str, fingerprint: &str) -> StoredDigest {
        StoredDigest {
            content: content.to_string(),
            fingerprint: fingerprint.to_string(),
            generated_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        }
    }

    #[test]
    fn a_digest_round_trips() {
        let fixture = fixture();
        let written = digest("## Tooling\n- pnpm", "3:2026-01-01T00:00:00+00:00");

        fixture
            .store
            .save(&fixture.alex, Domain::Coding, &written)
            .unwrap();

        assert_eq!(
            fixture.store.find(&fixture.alex, Domain::Coding).unwrap(),
            Some(written)
        );
    }

    #[test]
    fn a_missing_digest_is_absent_rather_than_an_error() {
        // The first read for every user, and for every user who has
        // never had a provider configured.
        let fixture = fixture();

        assert_eq!(
            fixture.store.find(&fixture.alex, Domain::Coding).unwrap(),
            None
        );
    }

    #[test]
    fn the_two_domains_are_stored_independently() {
        // The point of splitting them: writing one must not touch the
        // other.
        let fixture = fixture();

        fixture
            .store
            .save(&fixture.alex, Domain::Coding, &digest("coding", "a"))
            .unwrap();
        fixture
            .store
            .save(&fixture.alex, Domain::Personal, &digest("personal", "b"))
            .unwrap();

        assert_eq!(
            fixture
                .store
                .find(&fixture.alex, Domain::Coding)
                .unwrap()
                .unwrap()
                .content,
            "coding"
        );
        assert_eq!(
            fixture
                .store
                .find(&fixture.alex, Domain::Personal)
                .unwrap()
                .unwrap()
                .content,
            "personal"
        );
    }

    #[test]
    fn regenerating_replaces_rather_than_accumulating() {
        let fixture = fixture();

        fixture
            .store
            .save(&fixture.alex, Domain::Coding, &digest("old", "a"))
            .unwrap();
        fixture
            .store
            .save(&fixture.alex, Domain::Coding, &digest("new", "b"))
            .unwrap();

        let stored = fixture
            .store
            .find(&fixture.alex, Domain::Coding)
            .unwrap()
            .unwrap();
        assert_eq!(stored.content, "new");
        assert_eq!(stored.fingerprint, "b");
    }

    #[test]
    fn an_empty_digest_is_stored_rather_than_treated_as_absent() {
        // "Nothing worth saying" is a real answer, and caching it is
        // what stops it being re-asked on every session start.
        let fixture = fixture();

        fixture
            .store
            .save(&fixture.alex, Domain::Coding, &digest("", "a"))
            .unwrap();

        let stored = fixture.store.find(&fixture.alex, Domain::Coding).unwrap();
        assert!(stored.is_some(), "an empty digest was not cached");
        assert_eq!(stored.unwrap().content, "");
    }

    #[test]
    fn one_users_digest_is_invisible_to_another() {
        // This row summarises a person's entire memory store.
        let fixture = fixture();

        fixture
            .store
            .save(
                &fixture.alex,
                Domain::Coding,
                &digest("alex's profile", "a"),
            )
            .unwrap();

        assert_eq!(
            fixture.store.find(&fixture.sam, Domain::Coding).unwrap(),
            None,
            "another user's digest was returned"
        );
    }

    #[test]
    fn saving_never_overwrites_another_users_digest() {
        let fixture = fixture();
        fixture
            .store
            .save(&fixture.alex, Domain::Coding, &digest("alex's", "a"))
            .unwrap();

        fixture
            .store
            .save(&fixture.sam, Domain::Coding, &digest("sam's", "b"))
            .unwrap();

        assert_eq!(
            fixture
                .store
                .find(&fixture.alex, Domain::Coding)
                .unwrap()
                .unwrap()
                .content,
            "alex's"
        );
    }
}
