//! Tests for the SQLite memory repository and vector index.
//!
//! They share a fixture, so they live together rather than in each
//! adapter's own file: the interesting assertions are about the two
//! staying consistent with each other.

use super::sqlite_entity_graph::SqliteEntityGraph;
use super::sqlite_memory_repository::SqliteMemoryRepository;
use super::sqlite_vector_index::SqliteVectorIndex;
use super::tantivy_text_index::TantivyTextIndex;
use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::category::Category;
use crate::memories::domain::entity_graph::{EntityGraph, Relation};
use crate::memories::domain::entity_key::EntityKey;
use crate::memories::domain::memory::{Entity, Memory, MemorySource, NewMemory};
use crate::memories::domain::memory_repository::{AuditOperation, MemoryRepository};
use crate::memories::domain::text_index::TextIndex;
use crate::memories::domain::vector_index::VectorIndex;
use crate::shared::error::RaError;
use crate::shared::ids::MemoryId;
use crate::shared::sqlite::SqliteDatabase;
use chrono::{DateTime, Utc};
use std::sync::Arc;

const DIMENSIONS: usize = 4;

struct Fixture {
    memories: SqliteMemoryRepository,
    vectors: SqliteVectorIndex,
    text: TantivyTextIndex,
    graph: SqliteEntityGraph,
    alex: UserContext,
    sam: UserContext,
    /// The shared connection, for the few graph tests that assert on raw
    /// rows or install a trigger to induce a mid-transaction failure.
    database: Arc<SqliteDatabase>,
    // Holds the text index's directory for the test's lifetime.
    _index_dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
    let identity =
        crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();
    let index_dir = tempfile::tempdir().unwrap();

    Fixture {
        memories: SqliteMemoryRepository::new(Arc::clone(&database), "test-model", DIMENSIONS),
        vectors: SqliteVectorIndex::open(Arc::clone(&database), DIMENSIONS).unwrap(),
        text: TantivyTextIndex::open(index_dir.path().to_path_buf()).unwrap(),
        graph: SqliteEntityGraph::new(Arc::clone(&database)),
        alex: authenticate(&identity, "alex"),
        sam: authenticate(&identity, "sam"),
        database,
        _index_dir: index_dir,
    }
}

/// Builds a real `UserContext` the only way anything can — by
/// authenticating. Tests cannot forge one, which is the point.
fn authenticate(identity: &crate::bootstrap::wiring::Identity, handle: &str) -> UserContext {
    identity.user_creator.execute(handle, None).unwrap();
    let issued = identity
        .api_key_issuer
        .execute(
            handle,
            vec![crate::identity::domain::scope::Scope::Admin],
            "test",
        )
        .unwrap();
    identity
        .key_authenticator
        .execute(&issued.token.render())
        .unwrap()
}

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000, 0).unwrap()
}

fn memory_for(context: &UserContext, content: &str) -> Memory {
    Memory::create(
        context.user_id(),
        NewMemory {
            content: content.to_string(),
            category: Category::PreferenceCoding,
            subcategory: None,
            tags: vec!["typescript".to_string()],
            entities: vec![],
            confidence: 0.9,
            source: MemorySource {
                client: Some("test".to_string()),
                session_id: None,
            },
            expires_at: None,
        },
        now(),
    )
    .unwrap()
}

#[test]
fn round_trips_a_memory_with_every_field() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "User forbids barrel files");
    fixture
        .memories
        .insert(&fixture.alex, &memory, "test")
        .unwrap();

    let found = fixture
        .memories
        .find(&fixture.alex, memory.id())
        .unwrap()
        .unwrap();

    assert_eq!(found.id(), memory.id());
    assert_eq!(found.content(), "User forbids barrel files");
    assert_eq!(found.category(), &Category::PreferenceCoding);
    assert_eq!(found.tags(), &["typescript".to_string()]);
    assert_eq!(found.confidence(), 0.9);
    assert_eq!(found.source().client.as_deref(), Some("test"));
    assert_eq!(found.created_at(), now());
    assert!(!found.is_superseded());
}

#[test]
fn a_missing_memory_is_none_not_an_error() {
    let fixture = fixture();
    assert!(
        fixture
            .memories
            .find(&fixture.alex, MemoryId::new())
            .unwrap()
            .is_none()
    );
}

#[test]
fn one_user_cannot_read_anothers_memory_by_id() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "alex's secret");
    fixture
        .memories
        .insert(&fixture.alex, &memory, "test")
        .unwrap();

    // Sam knows the exact id and asks for it directly.
    assert!(
        fixture
            .memories
            .find(&fixture.sam, memory.id())
            .unwrap()
            .is_none(),
        "another user's memory was readable by id"
    );
}

#[test]
fn one_user_cannot_update_or_delete_anothers_memory() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "alex's memory");
    fixture
        .memories
        .insert(&fixture.alex, &memory, "test")
        .unwrap();

    let hijacked = memory
        .clone()
        .edit(
            crate::memories::domain::memory::MemoryEdit {
                content: Some("overwritten by sam".to_string()),
                ..Default::default()
            },
            now(),
        )
        .unwrap();

    assert!(matches!(
        fixture.memories.update(&fixture.sam, &hijacked, "sam"),
        Err(RaError::NotFound(_))
    ));
    assert!(matches!(
        fixture
            .memories
            .delete(&fixture.sam, memory.id(), "sam", ""),
        Err(RaError::NotFound(_))
    ));

    let untouched = fixture
        .memories
        .find(&fixture.alex, memory.id())
        .unwrap()
        .unwrap();
    assert_eq!(untouched.content(), "alex's memory");
}

#[test]
fn listing_returns_only_the_callers_memories() {
    let fixture = fixture();
    fixture
        .memories
        .insert(
            &fixture.alex,
            &memory_for(&fixture.alex, "alex one"),
            "test",
        )
        .unwrap();
    fixture
        .memories
        .insert(
            &fixture.alex,
            &memory_for(&fixture.alex, "alex two"),
            "test",
        )
        .unwrap();
    fixture
        .memories
        .insert(&fixture.sam, &memory_for(&fixture.sam, "sam one"), "test")
        .unwrap();

    let alex_memories = fixture.memories.list(&fixture.alex, false).unwrap();
    assert_eq!(alex_memories.len(), 2);
    assert!(
        alex_memories
            .iter()
            .all(|memory| memory.user_id() == fixture.alex.user_id())
    );
    assert_eq!(fixture.memories.list(&fixture.sam, false).unwrap().len(), 1);
}

#[test]
fn find_many_silently_drops_ids_belonging_to_another_user() {
    let fixture = fixture();
    let alex_memory = memory_for(&fixture.alex, "alex's");
    let sam_memory = memory_for(&fixture.sam, "sam's");
    fixture
        .memories
        .insert(&fixture.alex, &alex_memory, "t")
        .unwrap();
    fixture
        .memories
        .insert(&fixture.sam, &sam_memory, "t")
        .unwrap();

    let found = fixture
        .memories
        .find_many(&fixture.alex, &[alex_memory.id(), sam_memory.id()])
        .unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id(), alex_memory.id());
}

#[test]
fn a_deleted_memory_disappears_from_reads_but_stays_in_the_audit_trail() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "temporary");
    fixture
        .memories
        .insert(&fixture.alex, &memory, "test")
        .unwrap();

    fixture
        .memories
        .delete(&fixture.alex, memory.id(), "test", "")
        .unwrap();

    assert!(
        fixture
            .memories
            .find(&fixture.alex, memory.id())
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .memories
            .list(&fixture.alex, true)
            .unwrap()
            .is_empty()
    );

    let audit = fixture.memories.audit_trail(&fixture.alex, 10).unwrap();
    assert!(
        audit.iter().any(
            |entry| entry.memory_id == memory.id() && entry.operation == AuditOperation::Delete
        ),
        "the delete is missing from the audit trail: {audit:?}"
    );
}

#[test]
fn deleting_a_missing_memory_is_not_found() {
    let fixture = fixture();
    assert!(matches!(
        fixture
            .memories
            .delete(&fixture.alex, MemoryId::new(), "t", ""),
        Err(RaError::NotFound(_))
    ));
}

#[test]
fn superseded_memories_are_hidden_unless_asked_for() {
    let fixture = fixture();
    let old = memory_for(&fixture.alex, "deploys on fly.io");
    let new = memory_for(&fixture.alex, "deploys on hetzner");
    fixture
        .memories
        .insert(&fixture.alex, &old, "test")
        .unwrap();
    fixture
        .memories
        .insert(&fixture.alex, &new, "test")
        .unwrap();

    let superseded = old.clone().supersede(new.id(), now());
    fixture
        .memories
        .update(&fixture.alex, &superseded, "test")
        .unwrap();

    let active = fixture.memories.list(&fixture.alex, false).unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id(), new.id());

    let all = fixture.memories.list(&fixture.alex, true).unwrap();
    assert_eq!(all.len(), 2, "supersede must retain the memory");
    let reloaded = all.iter().find(|m| m.id() == old.id()).unwrap();
    assert_eq!(reloaded.superseded_by(), Some(new.id()));
    assert_eq!(reloaded.content(), "deploys on fly.io");
}

#[test]
fn every_mutation_is_audited() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "original");
    fixture
        .memories
        .insert(&fixture.alex, &memory, "cli")
        .unwrap();

    let edited = memory
        .clone()
        .edit(
            crate::memories::domain::memory::MemoryEdit {
                content: Some("revised".to_string()),
                ..Default::default()
            },
            now(),
        )
        .unwrap();
    fixture
        .memories
        .update(&fixture.alex, &edited, "rest")
        .unwrap();
    fixture
        .memories
        .delete(&fixture.alex, memory.id(), "mcp", "")
        .unwrap();

    let audit = fixture.memories.audit_trail(&fixture.alex, 10).unwrap();
    let operations: Vec<AuditOperation> = audit.iter().map(|entry| entry.operation).collect();

    assert!(operations.contains(&AuditOperation::Add));
    assert!(operations.contains(&AuditOperation::Update));
    assert!(operations.contains(&AuditOperation::Delete));
    assert!(
        audit.iter().any(|entry| entry.actor == "mcp"),
        "the actor should be recorded"
    );
}

#[test]
fn the_audit_trail_is_per_user() {
    let fixture = fixture();
    fixture
        .memories
        .insert(&fixture.alex, &memory_for(&fixture.alex, "alex"), "test")
        .unwrap();

    assert!(
        fixture
            .memories
            .audit_trail(&fixture.sam, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .memories
            .audit_trail(&fixture.alex, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn touching_records_access_only_for_the_callers_memories() {
    let fixture = fixture();
    let alex_memory = memory_for(&fixture.alex, "alex's");
    let sam_memory = memory_for(&fixture.sam, "sam's");
    fixture
        .memories
        .insert(&fixture.alex, &alex_memory, "t")
        .unwrap();
    fixture
        .memories
        .insert(&fixture.sam, &sam_memory, "t")
        .unwrap();

    let later = now() + chrono::Duration::hours(2);
    fixture
        .memories
        .touch_accessed(&fixture.alex, &[alex_memory.id(), sam_memory.id()], later)
        .unwrap();

    assert_eq!(
        fixture
            .memories
            .find(&fixture.alex, alex_memory.id())
            .unwrap()
            .unwrap()
            .last_accessed_at(),
        Some(later)
    );
    assert_eq!(
        fixture
            .memories
            .find(&fixture.sam, sam_memory.id())
            .unwrap()
            .unwrap()
            .last_accessed_at(),
        None,
        "touching leaked across users"
    );
}

#[test]
fn a_changed_embedding_model_is_refused_rather_than_silently_mixed() {
    let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
    let identity =
        crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();
    let alex = authenticate(&identity, "alex");

    let original = SqliteMemoryRepository::new(Arc::clone(&database), "model-a", DIMENSIONS);
    original
        .insert(&alex, &memory_for(&alex, "written with model a"), "t")
        .unwrap();

    // Same data directory, different configured model.
    let reconfigured = SqliteMemoryRepository::new(Arc::clone(&database), "model-b", DIMENSIONS);
    let error = reconfigured
        .insert(&alex, &memory_for(&alex, "written with model b"), "t")
        .unwrap_err();

    assert!(matches!(error, RaError::Validation(_)), "got {error:?}");
    let message = error.to_string();
    assert!(
        message.contains("model-a") && message.contains("model-b"),
        "{message}"
    );
    assert!(
        message.contains("recuerdos-ai reindex"),
        "should name the command that fixes it: {message}"
    );
}

#[test]
fn verify_pin_catches_a_dimension_change_at_startup_not_on_the_first_recall() {
    // The scenario behind the raw "Expected 384 dimensions but received
    // 3072" error: a store built by one model, reopened under another of a
    // different width. `verify_pin` must catch it up front — the recall
    // path never runs the write-path guard, so without this the mismatch
    // reaches sqlite-vec.
    let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
    let identity =
        crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();
    let alex = authenticate(&identity, "alex");

    // A store pinned to a 384-dim model.
    let original = SqliteMemoryRepository::new(Arc::clone(&database), "small", 384);
    original
        .insert(&alex, &memory_for(&alex, "written by the small model"), "t")
        .unwrap();

    // Same store, now configured for a 3072-dim model.
    let reconfigured = SqliteMemoryRepository::new(Arc::clone(&database), "gemini-embedding", 3072);
    let error = reconfigured.verify_pin().unwrap_err();

    assert!(matches!(error, RaError::Validation(_)), "got {error:?}");
    let message = error.to_string();
    assert!(
        message.contains("384") && message.contains("3072"),
        "{message}"
    );
    assert!(
        message.contains("recuerdos-ai reindex"),
        "should name the fix: {message}"
    );
}

#[test]
fn verify_pin_is_happy_with_a_fresh_store_and_a_matching_one() {
    let database = Arc::new(SqliteDatabase::open_in_memory().unwrap());
    let identity =
        crate::bootstrap::wiring::Identity::from_database(Arc::clone(&database)).unwrap();
    let alex = authenticate(&identity, "alex");

    let repository = SqliteMemoryRepository::new(Arc::clone(&database), "test-model", DIMENSIONS);
    // Fresh: nothing pinned yet.
    repository
        .verify_pin()
        .expect("an empty store has no pin to conflict with");

    // After a write, the pin exists and still matches the same config.
    repository
        .insert(&alex, &memory_for(&alex, "hello"), "t")
        .unwrap();
    repository
        .verify_pin()
        .expect("the pin matches the configured model");
}

// ---- vector index ----

#[test]
fn vector_search_returns_the_nearest_first() {
    let fixture = fixture();
    let near = memory_for(&fixture.alex, "near");
    let far = memory_for(&fixture.alex, "far");

    fixture
        .vectors
        .upsert(&fixture.alex, near.id(), &[1.0, 0.0, 0.0, 0.0])
        .unwrap();
    fixture
        .vectors
        .upsert(&fixture.alex, far.id(), &[0.0, 1.0, 0.0, 0.0])
        .unwrap();

    let hits = fixture
        .vectors
        .search(&fixture.alex, &[0.9, 0.1, 0.0, 0.0], 10)
        .unwrap();

    assert_eq!(hits.first(), Some(&near.id()));
    assert_eq!(hits.len(), 2);
}

#[test]
fn vector_search_never_crosses_users_even_for_identical_vectors() {
    let fixture = fixture();
    let alex_memory = memory_for(&fixture.alex, "alex");
    let sam_memory = memory_for(&fixture.sam, "sam");

    // The same vector for both: only the partition key separates them.
    let vector = [1.0, 0.0, 0.0, 0.0];
    fixture
        .vectors
        .upsert(&fixture.alex, alex_memory.id(), &vector)
        .unwrap();
    fixture
        .vectors
        .upsert(&fixture.sam, sam_memory.id(), &vector)
        .unwrap();

    let alex_hits = fixture.vectors.search(&fixture.alex, &vector, 10).unwrap();
    assert_eq!(alex_hits, vec![alex_memory.id()]);

    let sam_hits = fixture.vectors.search(&fixture.sam, &vector, 10).unwrap();
    assert_eq!(sam_hits, vec![sam_memory.id()]);
}

#[test]
fn upserting_the_same_memory_replaces_its_vector() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "moving target");

    fixture
        .vectors
        .upsert(&fixture.alex, memory.id(), &[1.0, 0.0, 0.0, 0.0])
        .unwrap();
    fixture
        .vectors
        .upsert(&fixture.alex, memory.id(), &[0.0, 1.0, 0.0, 0.0])
        .unwrap();

    let hits = fixture
        .vectors
        .search(&fixture.alex, &[0.0, 1.0, 0.0, 0.0], 10)
        .unwrap();

    assert_eq!(hits, vec![memory.id()], "the old vector was left behind");
}

#[test]
fn removing_a_vector_takes_it_out_of_search() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "transient");
    fixture
        .vectors
        .upsert(&fixture.alex, memory.id(), &[1.0, 0.0, 0.0, 0.0])
        .unwrap();

    fixture.vectors.remove(&fixture.alex, memory.id()).unwrap();

    assert!(
        fixture
            .vectors
            .search(&fixture.alex, &[1.0, 0.0, 0.0, 0.0], 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn one_user_cannot_remove_anothers_vector() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "alex's");
    let vector = [1.0, 0.0, 0.0, 0.0];
    fixture
        .vectors
        .upsert(&fixture.alex, memory.id(), &vector)
        .unwrap();

    fixture.vectors.remove(&fixture.sam, memory.id()).unwrap();

    assert_eq!(
        fixture.vectors.search(&fixture.alex, &vector, 10).unwrap(),
        vec![memory.id()],
        "another user's remove deleted this vector"
    );
}

#[test]
fn a_wrong_sized_embedding_is_rejected_rather_than_stored() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "x");

    let error = fixture
        .vectors
        .upsert(&fixture.alex, memory.id(), &[1.0, 0.0])
        .unwrap_err();

    assert!(error.to_string().contains("dimensions"), "{error}");
}

#[test]
fn searching_an_empty_index_returns_nothing() {
    let fixture = fixture();
    assert!(
        fixture
            .vectors
            .search(&fixture.alex, &[1.0, 0.0, 0.0, 0.0], 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_zero_limit_returns_nothing_rather_than_erroring() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "x");
    fixture
        .vectors
        .upsert(&fixture.alex, memory.id(), &[1.0, 0.0, 0.0, 0.0])
        .unwrap();

    assert!(
        fixture
            .vectors
            .search(&fixture.alex, &[1.0, 0.0, 0.0, 0.0], 0)
            .unwrap()
            .is_empty()
    );
}

// ---- text index ----

#[test]
fn text_search_finds_a_memory_by_its_words() {
    let fixture = fixture();
    let memory = memory_for(
        &fixture.alex,
        "User forbids barrel files and index re-exports",
    );
    fixture.text.upsert(&fixture.alex, &memory).unwrap();

    let hits = fixture
        .text
        .search(&fixture.alex, "barrel files", 10)
        .unwrap();

    assert_eq!(hits, vec![memory.id()]);
}

#[test]
fn text_search_finds_an_exact_identifier_a_vector_would_blur() {
    // The case that justifies having a keyword leg at all
    // (project-plan.md §7.7). An embedding places `useQuery` near its
    // semantic neighbours — `useState`, `useEffect`, "react hook" — so
    // vector search alone answers "which hook?" rather than "this exact
    // symbol". BM25 matches the literal token.
    let fixture = fixture();
    let target = memory_for(
        &fixture.alex,
        "The useQuery cache key must include the tenant id",
    );
    let neighbour = memory_for(
        &fixture.alex,
        "Prefer useState over useReducer for simple state",
    );
    fixture.text.upsert(&fixture.alex, &target).unwrap();
    fixture.text.upsert(&fixture.alex, &neighbour).unwrap();

    let hits = fixture.text.search(&fixture.alex, "useQuery", 10).unwrap();

    assert_eq!(
        hits.first(),
        Some(&target.id()),
        "the exact identifier should win"
    );
}

#[test]
fn text_search_matches_tags_and_category_too() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "content without the word");
    fixture.text.upsert(&fixture.alex, &memory).unwrap();

    assert_eq!(
        fixture
            .text
            .search(&fixture.alex, "typescript", 10)
            .unwrap(),
        vec![memory.id()],
        "tags should be searchable"
    );
    assert_eq!(
        fixture
            .text
            .search(&fixture.alex, "preference.coding", 10)
            .unwrap(),
        vec![memory.id()],
        "category should be searchable"
    );
}

#[test]
fn text_search_never_crosses_users() {
    let fixture = fixture();
    let alex_memory = memory_for(&fixture.alex, "shared vocabulary about pnpm");
    let sam_memory = memory_for(&fixture.sam, "shared vocabulary about pnpm");
    fixture.text.upsert(&fixture.alex, &alex_memory).unwrap();
    fixture.text.upsert(&fixture.sam, &sam_memory).unwrap();

    assert_eq!(
        fixture.text.search(&fixture.alex, "pnpm", 10).unwrap(),
        vec![alex_memory.id()]
    );
    assert_eq!(
        fixture.text.search(&fixture.sam, "pnpm", 10).unwrap(),
        vec![sam_memory.id()]
    );
}

#[test]
fn reindexing_a_memory_does_not_duplicate_it() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "original wording about pnpm");
    fixture.text.upsert(&fixture.alex, &memory).unwrap();
    fixture.text.upsert(&fixture.alex, &memory).unwrap();

    let hits = fixture.text.search(&fixture.alex, "pnpm", 10).unwrap();

    assert_eq!(
        hits.len(),
        1,
        "tantivy has no update; upsert must delete first"
    );
}

#[test]
fn editing_a_memory_makes_the_new_words_findable_and_the_old_ones_not() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "deploys on flyio");
    fixture.text.upsert(&fixture.alex, &memory).unwrap();

    let edited = memory
        .clone()
        .edit(
            crate::memories::domain::memory::MemoryEdit {
                content: Some("deploys on hetzner".to_string()),
                ..Default::default()
            },
            now(),
        )
        .unwrap();
    fixture.text.upsert(&fixture.alex, &edited).unwrap();

    assert_eq!(
        fixture.text.search(&fixture.alex, "hetzner", 10).unwrap(),
        vec![memory.id()]
    );
    assert!(
        fixture
            .text
            .search(&fixture.alex, "flyio", 10)
            .unwrap()
            .is_empty(),
        "the superseded wording is still indexed"
    );
}

#[test]
fn removing_a_memory_takes_it_out_of_text_search() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "transient note about pnpm");
    fixture.text.upsert(&fixture.alex, &memory).unwrap();

    fixture.text.remove(&fixture.alex, memory.id()).unwrap();

    assert!(
        fixture
            .text
            .search(&fixture.alex, "pnpm", 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn one_user_cannot_remove_anothers_indexed_memory() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "alex's note about pnpm");
    fixture.text.upsert(&fixture.alex, &memory).unwrap();

    fixture.text.remove(&fixture.sam, memory.id()).unwrap();

    assert_eq!(
        fixture.text.search(&fixture.alex, "pnpm", 10).unwrap(),
        vec![memory.id()],
        "another user's remove deleted this document"
    );
}

#[test]
fn a_natural_language_question_matches_on_any_of_its_words() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "User prefers pnpm as their package manager");
    fixture.text.upsert(&fixture.alex, &memory).unwrap();

    // Requiring every term would match nothing here.
    let hits = fixture
        .text
        .search(&fixture.alex, "which package manager should I use?", 10)
        .unwrap();

    assert_eq!(hits, vec![memory.id()]);
}

#[test]
fn punctuation_heavy_input_does_not_error() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "note about pnpm");
    fixture.text.upsert(&fixture.alex, &memory).unwrap();

    for query in ["+++", "pnpm:", "\"unclosed", "a/b\\c", "((("] {
        let result = fixture.text.search(&fixture.alex, query, 10);
        assert!(result.is_ok(), "{query:?} produced {result:?}");
    }
}

#[test]
fn searching_an_empty_query_or_zero_limit_returns_nothing() {
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "note about pnpm");
    fixture.text.upsert(&fixture.alex, &memory).unwrap();

    assert!(
        fixture
            .text
            .search(&fixture.alex, "   ", 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .text
            .search(&fixture.alex, "pnpm", 0)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn searching_a_user_with_no_index_yet_is_empty_not_an_error() {
    let fixture = fixture();
    assert!(
        fixture
            .text
            .search(&fixture.sam, "anything", 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn each_user_gets_their_own_index_directory() {
    let fixture = fixture();
    fixture
        .text
        .upsert(&fixture.alex, &memory_for(&fixture.alex, "alex"))
        .unwrap();
    fixture
        .text
        .upsert(&fixture.sam, &memory_for(&fixture.sam, "sam"))
        .unwrap();

    let alex_dir = fixture
        ._index_dir
        .path()
        .join(fixture.alex.user_id().to_string());
    let sam_dir = fixture
        ._index_dir
        .path()
        .join(fixture.sam.user_id().to_string());

    assert!(alex_dir.is_dir(), "expected a per-user index directory");
    assert!(sam_dir.is_dir());
}

#[test]
fn merging_retires_a_whole_cluster_and_the_trail_reads_back() {
    // The read-back half is the point. Every audit operation is written
    // as a string and parsed on the way out, and the two lists live in
    // different files — so an operation that writes fine and cannot be
    // read turns `GET /v1/audit` into a 500 for that user, permanently,
    // for every entry after it.
    let fixture = fixture();
    let cluster: Vec<Memory> = (0..3)
        .map(|index| {
            let memory = memory_for(&fixture.alex, &format!("phrasing {index}"));
            fixture
                .memories
                .insert(&fixture.alex, &memory, "test")
                .unwrap();
            memory
        })
        .collect();
    let replacement = memory_for(&fixture.alex, "the merged memory");
    fixture
        .memories
        .insert(&fixture.alex, &replacement, "consolidation")
        .unwrap();

    let ids: Vec<MemoryId> = cluster.iter().map(Memory::id).collect();
    let retired = fixture
        .memories
        .merge(
            &fixture.alex,
            &ids,
            replacement.id(),
            "consolidation",
            "three phrasings of one preference",
        )
        .unwrap();

    assert_eq!(retired, 3);
    for id in &ids {
        let stored = fixture.memories.find(&fixture.alex, *id).unwrap().unwrap();
        assert_eq!(
            stored.superseded_by(),
            Some(replacement.id()),
            "a cluster member was not retired"
        );
    }

    let trail = fixture.memories.audit_trail(&fixture.alex, 100).unwrap();
    let merges: Vec<_> = trail
        .iter()
        .filter(|entry| entry.operation == AuditOperation::Merge)
        .collect();
    assert_eq!(merges.len(), 3);
    assert!(merges[0].detail.contains("three phrasings"));
}

#[test]
fn merging_skips_members_another_user_owns() {
    // A cluster is built from one user's memories, but the repository is
    // the last line of defence and must not take an id on trust.
    let fixture = fixture();
    let theirs = memory_for(&fixture.sam, "sam's memory");
    fixture
        .memories
        .insert(&fixture.sam, &theirs, "test")
        .unwrap();
    let replacement = memory_for(&fixture.alex, "alex's merged memory");
    fixture
        .memories
        .insert(&fixture.alex, &replacement, "test")
        .unwrap();

    let retired = fixture
        .memories
        .merge(
            &fixture.alex,
            &[theirs.id()],
            replacement.id(),
            "consolidation",
            "should not happen",
        )
        .unwrap();

    assert_eq!(retired, 0, "another user's memory was retired");
    assert!(
        !fixture
            .memories
            .find(&fixture.sam, theirs.id())
            .unwrap()
            .unwrap()
            .is_superseded()
    );
}

#[test]
fn merging_skips_a_member_that_is_already_retired() {
    // Clusters are snapshots. Re-pointing a memory something else already
    // superseded would rewrite history.
    let fixture = fixture();
    let first = memory_for(&fixture.alex, "already merged away");
    let earlier = memory_for(&fixture.alex, "the earlier replacement");
    let replacement = memory_for(&fixture.alex, "the new replacement");
    for memory in [&first, &earlier, &replacement] {
        fixture
            .memories
            .insert(&fixture.alex, memory, "test")
            .unwrap();
    }
    fixture
        .memories
        .supersede(&fixture.alex, first.id(), earlier.id(), "test", "")
        .unwrap();

    let retired = fixture
        .memories
        .merge(
            &fixture.alex,
            &[first.id()],
            replacement.id(),
            "consolidation",
            "",
        )
        .unwrap();

    assert_eq!(retired, 0);
    assert_eq!(
        fixture
            .memories
            .find(&fixture.alex, first.id())
            .unwrap()
            .unwrap()
            .superseded_by(),
        Some(earlier.id()),
        "an already-retired memory was re-pointed"
    );
}

#[test]
fn recall_bookkeeping_accumulates_across_calls() {
    // Decay's only inputs. A stamp that overwrote the count, or a count
    // that reset, would make every memory look equally unused and the
    // nightly rescore a no-op nobody would notice.
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "a memory");
    fixture
        .memories
        .insert(&fixture.alex, &memory, "test")
        .unwrap();

    for hour in 1..=3 {
        fixture
            .memories
            .touch_accessed(
                &fixture.alex,
                &[memory.id()],
                now() + chrono::Duration::hours(hour),
            )
            .unwrap();
    }

    let stored = fixture
        .memories
        .find(&fixture.alex, memory.id())
        .unwrap()
        .unwrap();
    assert_eq!(stored.access_count(), 3);
    assert_eq!(
        stored.last_accessed_at(),
        Some(now() + chrono::Duration::hours(3)),
        "the stamp should be the most recent access, not the first"
    );
}

#[test]
fn touching_never_reaches_another_users_memory() {
    let fixture = fixture();
    let theirs = memory_for(&fixture.sam, "sam's memory");
    fixture
        .memories
        .insert(&fixture.sam, &theirs, "test")
        .unwrap();

    fixture
        .memories
        .touch_accessed(&fixture.alex, &[theirs.id()], now())
        .unwrap();

    let stored = fixture
        .memories
        .find(&fixture.sam, theirs.id())
        .unwrap()
        .unwrap();
    assert_eq!(stored.access_count(), 0);
    assert_eq!(stored.last_accessed_at(), None);
}

#[test]
fn a_new_memory_starts_fully_important_and_round_trips_its_score() {
    // The default matters: a memory written between two nightly runs
    // must rank normally rather than being buried until something has
    // measured it.
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "a memory");
    fixture
        .memories
        .insert(&fixture.alex, &memory, "test")
        .unwrap();

    let fresh = fixture
        .memories
        .find(&fixture.alex, memory.id())
        .unwrap()
        .unwrap();
    assert_eq!(fresh.importance(), 1.0);
    assert_eq!(fresh.access_count(), 0);

    fixture
        .memories
        .set_importance(&fixture.alex, &[(memory.id(), 0.42)])
        .unwrap();

    let rescored = fixture
        .memories
        .find(&fixture.alex, memory.id())
        .unwrap()
        .unwrap();
    assert!((rescored.importance() - 0.42).abs() < 1e-6);
}

#[test]
fn rescoring_never_reaches_another_users_memory() {
    let fixture = fixture();
    let theirs = memory_for(&fixture.sam, "sam's memory");
    fixture
        .memories
        .insert(&fixture.sam, &theirs, "test")
        .unwrap();

    fixture
        .memories
        .set_importance(&fixture.alex, &[(theirs.id(), 0.1)])
        .unwrap();

    let stored = fixture
        .memories
        .find(&fixture.sam, theirs.id())
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.importance(),
        1.0,
        "another user's score was rewritten"
    );
}

#[test]
fn rescoring_leaves_the_audit_trail_alone() {
    // A derived value, not a change anyone made. An entry per memory per
    // night would bury the changes a user actually cares about.
    let fixture = fixture();
    let memory = memory_for(&fixture.alex, "a memory");
    fixture
        .memories
        .insert(&fixture.alex, &memory, "test")
        .unwrap();
    let before = fixture
        .memories
        .audit_trail(&fixture.alex, 100)
        .unwrap()
        .len();

    fixture
        .memories
        .set_importance(&fixture.alex, &[(memory.id(), 0.5)])
        .unwrap();

    assert_eq!(
        fixture
            .memories
            .audit_trail(&fixture.alex, 100)
            .unwrap()
            .len(),
        before
    );
}

// ─────────────────────────────────────────────────────────────────────
// The entity graph (migration V8, Task 7.3.1). These live here beside the
// vector and text index tests for the same reason those do: the graph is
// a third index over the same store, and the interesting properties are
// isolation and consistency, tested the same way.
// ─────────────────────────────────────────────────────────────────────

fn entity(name: &str, kind: &str) -> Entity {
    Entity {
        name: name.to_string(),
        kind: kind.to_string(),
    }
}

fn rel(subject: &str, predicate: &str, object: &str) -> Relation {
    Relation {
        subject: subject.to_string(),
        predicate: predicate.to_string(),
        object: object.to_string(),
    }
}

fn seed(name: &str) -> EntityKey {
    EntityKey::new(name)
}

fn count(fixture: &Fixture, table: &str, context: &UserContext, memory_id: MemoryId) -> i64 {
    fixture
        .database
        .with_connection(|connection| {
            Ok(connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE user_id = ?1 AND memory_id = ?2"),
                    rusqlite::params![context.user_id().to_string(), memory_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap())
        })
        .unwrap()
}

#[test]
fn a_memorys_entities_and_edges_round_trip_and_removing_it_takes_them_out() {
    let fixture = fixture();
    let memory_id = MemoryId::new();

    fixture
        .graph
        .record(
            &fixture.alex,
            memory_id,
            &[
                entity("billing service", "service"),
                entity("Meridian team", "team"),
            ],
            &[rel("billing service", "maintained_by", "Meridian team")],
            now(),
        )
        .unwrap();

    assert_eq!(
        count(&fixture, "memory_entities", &fixture.alex, memory_id),
        2
    );
    assert_eq!(
        count(&fixture, "memory_relations", &fixture.alex, memory_id),
        1
    );

    fixture.graph.remove(&fixture.alex, memory_id).unwrap();

    assert_eq!(
        count(&fixture, "memory_entities", &fixture.alex, memory_id),
        0,
        "a forgotten memory left its entities behind"
    );
    assert_eq!(
        count(&fixture, "memory_relations", &fixture.alex, memory_id),
        0,
        "a forgotten memory left its edges behind"
    );
}

#[test]
fn recording_replaces_rather_than_accumulates() {
    // An edit re-records the memory; the projection must match the memory
    // afterwards, not carry the union of both versions.
    let fixture = fixture();
    let memory_id = MemoryId::new();

    fixture
        .graph
        .record(
            &fixture.alex,
            memory_id,
            &[entity("Fly.io", "service")],
            &[rel("backend", "deploys_on", "Fly.io")],
            now(),
        )
        .unwrap();
    fixture
        .graph
        .record(
            &fixture.alex,
            memory_id,
            &[entity("Hetzner", "service")],
            &[rel("backend", "deploys_on", "Hetzner")],
            now(),
        )
        .unwrap();

    assert_eq!(
        count(&fixture, "memory_entities", &fixture.alex, memory_id),
        1
    );
    assert_eq!(
        count(&fixture, "memory_relations", &fixture.alex, memory_id),
        1
    );

    // And the surviving edge is the second one.
    let found = fixture
        .graph
        .neighbours(&fixture.alex, &[seed("hetzner")], 1, None, 10)
        .unwrap();
    assert!(found.contains(&memory_id));
    let gone = fixture
        .graph
        .neighbours(&fixture.alex, &[seed("fly.io")], 1, None, 10)
        .unwrap();
    assert!(gone.is_empty(), "the replaced edge is still reachable");
}

#[test]
fn a_two_hop_neighbour_is_out_of_reach_at_one_hop() {
    // billing service —maintained_by→ Meridian team ←leads— Nadia.
    // Seeding the billing service reaches the ownership edge in one hop
    // and Nadia only in two.
    let fixture = fixture();
    let owns = MemoryId::new();
    let leads = MemoryId::new();

    fixture
        .graph
        .record(
            &fixture.alex,
            owns,
            &[
                entity("billing service", "service"),
                entity("Meridian team", "team"),
            ],
            &[rel("billing service", "maintained_by", "Meridian team")],
            now(),
        )
        .unwrap();
    fixture
        .graph
        .record(
            &fixture.alex,
            leads,
            &[entity("Nadia", "person"), entity("Meridian team", "team")],
            &[rel("Nadia", "leads", "Meridian team")],
            now(),
        )
        .unwrap();

    let one_hop = fixture
        .graph
        .neighbours(&fixture.alex, &[seed("billing service")], 1, None, 10)
        .unwrap();
    assert!(
        one_hop.contains(&owns),
        "the ownership edge is one hop away"
    );
    assert!(
        !one_hop.contains(&leads),
        "Nadia is two hops away and must not appear at one"
    );

    let two_hops = fixture
        .graph
        .neighbours(&fixture.alex, &[seed("billing service")], 2, None, 10)
        .unwrap();
    assert!(two_hops.contains(&owns));
    assert!(
        two_hops.contains(&leads),
        "Nadia should be reachable in two hops"
    );
}

#[test]
fn a_hop_never_crosses_users_even_with_the_same_entity_name() {
    // Both users store an entity called "shared service". A hop for one
    // must never reach the other's edges — the graph's isolation is a
    // property of the WHERE clause, not of the entity names being distinct.
    let fixture = fixture();
    let alex_memory = MemoryId::new();
    let sam_memory = MemoryId::new();

    fixture
        .graph
        .record(
            &fixture.alex,
            alex_memory,
            &[
                entity("shared service", "service"),
                entity("alex thing", "thing"),
            ],
            &[rel("shared service", "relates_to", "alex thing")],
            now(),
        )
        .unwrap();
    fixture
        .graph
        .record(
            &fixture.sam,
            sam_memory,
            &[
                entity("shared service", "service"),
                entity("sam thing", "thing"),
            ],
            &[rel("shared service", "relates_to", "sam thing")],
            now(),
        )
        .unwrap();

    let alex_hits = fixture
        .graph
        .neighbours(&fixture.alex, &[seed("shared service")], 2, None, 10)
        .unwrap();
    assert!(alex_hits.contains(&alex_memory));
    assert!(
        !alex_hits.contains(&sam_memory),
        "alex's hop reached sam's edge"
    );

    let sam_hits = fixture
        .graph
        .neighbours(&fixture.sam, &[seed("shared service")], 2, None, 10)
        .unwrap();
    assert!(sam_hits.contains(&sam_memory));
    assert!(!sam_hits.contains(&alex_memory));

    // And a remove is scoped too: alex cannot delete sam's rows by naming
    // his memory id.
    fixture.graph.remove(&fixture.alex, sam_memory).unwrap();
    assert_eq!(
        count(&fixture, "memory_relations", &fixture.sam, sam_memory),
        1,
        "alex removed sam's edge"
    );
}

#[test]
fn an_empty_or_unknown_seed_reaches_nothing() {
    let fixture = fixture();
    let memory_id = MemoryId::new();
    fixture
        .graph
        .record(
            &fixture.alex,
            memory_id,
            &[
                entity("billing service", "service"),
                entity("Meridian team", "team"),
            ],
            &[rel("billing service", "maintained_by", "Meridian team")],
            now(),
        )
        .unwrap();

    assert!(
        fixture
            .graph
            .neighbours(&fixture.alex, &[], 2, None, 10)
            .unwrap()
            .is_empty(),
        "no seeds must mean no hop — this is what keeps a query naming no \
         known entity from perturbing recall"
    );
    assert!(
        fixture
            .graph
            .neighbours(&fixture.alex, &[seed("nothing here")], 2, None, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_failed_edge_write_rolls_back_the_entities_written_before_it() {
    // record() inserts a memory's entities and then its edges in one
    // transaction. Inducing the edge insert to fail must undo the entity
    // rows too, or a half-written projection would point at the memory.
    let fixture = fixture();
    fixture
        .database
        .with_connection(|connection| {
            connection
                .execute_batch(
                    "CREATE TEMP TRIGGER boom BEFORE INSERT ON memory_relations
                     BEGIN SELECT RAISE(ABORT, 'boom'); END;",
                )
                .unwrap();
            Ok(())
        })
        .unwrap();

    let memory_id = MemoryId::new();
    let result = fixture.graph.record(
        &fixture.alex,
        memory_id,
        &[entity("Hetzner", "service")],
        &[rel("backend", "deploys_on", "Hetzner")],
        now(),
    );

    assert!(result.is_err(), "the aborted edge write should surface");
    assert_eq!(
        count(&fixture, "memory_entities", &fixture.alex, memory_id),
        0,
        "the entity rows written before the failing edge were not rolled back"
    );
}

#[test]
fn an_edge_is_reachable_only_within_its_validity_interval() {
    // valid_from is when the fact became true; a read as_of an earlier
    // point must not see it. This is the bi-temporal read Task 7.3.3
    // builds on.
    let fixture = fixture();
    let memory_id = MemoryId::new();
    let born = now();
    fixture
        .graph
        .record(
            &fixture.alex,
            memory_id,
            &[entity("backend", "component"), entity("Fly.io", "service")],
            &[rel("backend", "deploys_on", "Fly.io")],
            born,
        )
        .unwrap();

    let before = born - chrono::Duration::days(1);
    let after = born + chrono::Duration::days(1);

    assert!(
        fixture
            .graph
            .neighbours(&fixture.alex, &[seed("backend")], 1, Some(before), 10)
            .unwrap()
            .is_empty(),
        "the edge was visible before it became true"
    );
    assert!(
        fixture
            .graph
            .neighbours(&fixture.alex, &[seed("backend")], 1, Some(after), 10)
            .unwrap()
            .contains(&memory_id)
    );
    assert!(
        fixture
            .graph
            .neighbours(&fixture.alex, &[seed("backend")], 1, None, 10)
            .unwrap()
            .contains(&memory_id),
        "with no as_of the current (still-open) edge should be reachable"
    );
}

#[test]
fn invalidation_closes_a_contradicted_edge_but_history_still_reads() {
    // backend deploys on Fly.io (from `born`), then a migration memory
    // re-asserts it as Hetzner. The Fly.io edge is closed as of the
    // migration, not deleted: a read from before still sees it.
    let fixture = fixture();
    let fly_memory = MemoryId::new();
    let hetzner_memory = MemoryId::new();
    let born = now();
    let migration = born + chrono::Duration::days(30);

    fixture
        .graph
        .record(
            &fixture.alex,
            fly_memory,
            &[entity("backend", "component"), entity("Fly.io", "service")],
            &[rel("backend", "deploys_on", "Fly.io")],
            born,
        )
        .unwrap();

    fixture
        .graph
        .invalidate(
            &fixture.alex,
            &[rel("backend", "deploys_on", "Hetzner")],
            migration,
            hetzner_memory,
        )
        .unwrap();

    // Current view: the Fly.io edge is gone.
    assert!(
        fixture
            .graph
            .neighbours(&fixture.alex, &[seed("backend")], 1, None, 10)
            .unwrap()
            .is_empty(),
        "the contradicted edge is still live in the current view"
    );
    // Historical view, between born and the migration: it is still there.
    let midpoint = born + chrono::Duration::days(15);
    assert!(
        fixture
            .graph
            .neighbours(&fixture.alex, &[seed("backend")], 1, Some(midpoint), 10)
            .unwrap()
            .contains(&fly_memory),
        "history was rewritten rather than preserved"
    );
}

#[test]
fn invalidation_spares_a_reaffirmed_edge_and_is_idempotent() {
    let fixture = fixture();
    let memory_id = MemoryId::new();
    let by = MemoryId::new();
    let born = now();
    fixture
        .graph
        .record(
            &fixture.alex,
            memory_id,
            &[entity("backend", "component"), entity("Hetzner", "service")],
            &[rel("backend", "deploys_on", "Hetzner")],
            born,
        )
        .unwrap();

    // Re-asserting the SAME object is not a contradiction — the edge stays.
    let at = born + chrono::Duration::days(1);
    fixture
        .graph
        .invalidate(
            &fixture.alex,
            &[rel("backend", "deploys_on", "Hetzner")],
            at,
            by,
        )
        .unwrap();
    // Running it again must not move anything either.
    fixture
        .graph
        .invalidate(
            &fixture.alex,
            &[rel("backend", "deploys_on", "Hetzner")],
            at,
            by,
        )
        .unwrap();

    assert!(
        fixture
            .graph
            .neighbours(&fixture.alex, &[seed("backend")], 1, None, 10)
            .unwrap()
            .contains(&memory_id),
        "a re-affirmed edge was wrongly closed"
    );
}

#[test]
fn the_default_build_has_no_graph_and_enabling_it_wires_one() {
    // The inert-by-default guarantee, at the wiring seam: recall never
    // consults a graph that isn't there, so a default build behaves
    // exactly as it did before Task 7.3.
    use crate::bootstrap::config::AppConfig;

    let off = AppConfig::default();
    assert!(!off.graph.enabled, "the graph must default to off");

    let mut on = AppConfig::default();
    on.graph.enabled = true;
    assert!(on.graph.enabled);
}
