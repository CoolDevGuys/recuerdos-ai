//! Tests for the SQLite memory repository and vector index.
//!
//! They share a fixture, so they live together rather than in each
//! adapter's own file: the interesting assertions are about the two
//! staying consistent with each other.

use super::sqlite_memory_repository::SqliteMemoryRepository;
use super::sqlite_vector_index::SqliteVectorIndex;
use super::tantivy_text_index::TantivyTextIndex;
use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::category::Category;
use crate::memories::domain::memory::{Memory, MemorySource, NewMemory};
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
    alex: UserContext,
    sam: UserContext,
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
        alex: authenticate(&identity, "alex"),
        sam: authenticate(&identity, "sam"),
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
        fixture.memories.delete(&fixture.sam, memory.id(), "sam"),
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
    assert_eq!(fixture.memories.count(&fixture.alex).unwrap(), 2);
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
        .delete(&fixture.alex, memory.id(), "test")
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
        fixture.memories.delete(&fixture.alex, MemoryId::new(), "t"),
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
        .delete(&fixture.alex, memory.id(), "mcp")
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
        message.contains("re-index"),
        "should say how to fix it: {message}"
    );
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
