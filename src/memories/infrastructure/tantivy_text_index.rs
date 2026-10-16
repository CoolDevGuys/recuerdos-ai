//! `TextIndex` backed by tantivy — the BM25 leg of hybrid search.
//!
//! # One index per user
//!
//! Each user gets their own directory under `<data>/text-index/v2/<user>/`
//! (the versioned root — see below).
//! A single shared index with a `user_id` field would work, but this way
//! isolation is structural: a query opens one user's directory and
//! physically cannot see another's postings, so there is no filter to
//! forget. It also keeps each index small, which is what BM25's
//! corpus-relative scoring wants.
//!
//! The cost is a writer per active user; they are created lazily and
//! held in a map. At personal-deployment scale (tens of users) that is
//! nothing. If this ever hosts thousands of users, a shared index with a
//! filter becomes the better trade — noted here as the thing to revisit.
//!
//! # Durability
//!
//! The index is *derived* state. Its system of record is the `memories`
//! table, and it can be rebuilt at any time. That is why a commit failure
//! here is not fatal to a write that already reached SQLite.
//!
//! # The analyzer, and why the index directory is versioned
//!
//! Text fields use the `ra_text_v2` analyzer: simple tokenization, a
//! length cap, lowercasing, and the project stopword list
//! (`stopwords.rs`). Function words are removed from *indexed text and
//! queries alike*, because the query parser tokenizes with the field's
//! analyzer — which is the entire point (reranking-plan.md §1: the
//! keyword leg was rewarding matches on "what", "do", "the").
//!
//! `Index::open_in_dir` reuses the *stored* schema, so an index created
//! before the analyzer existed would keep referencing `default` no
//! matter what this code registers. Rather than silently carrying that
//! defect, each analyzer change lives under a versioned directory
//! (`<root>/v2/<user>/`) and the old one is simply ignored; bump
//! `TEXT_INDEX_VERSION` with the next schema or analyzer change. A user
//! whose versioned directory is absent gets it from the startup
//! rebuild or `reindex --text` (see the rebuilder use case).
//!
//! Tokenizer registrations are per-`Index`-instance, *not* persisted in
//! the index, so `user_index` registers the analyzer on every open.
//!
//! # Read-only
//!
//! [`TantivyTextIndex::open_read_only`] opens the same directories without
//! ever creating an `IndexWriter` — tantivy takes an exclusive writer lock
//! per index directory, so a read-write open fails while the daemon is
//! serving that user. The audit path (reranking-plan.md Phase 6.2) reads a
//! live store, so it opens this way: searches work, and every mutation
//! returns an error instead of writing.

use super::stopwords::STOP_WORDS;
use crate::identity::domain::user_context::UserContext;
use crate::memories::domain::memory::Memory;
use crate::memories::domain::text_index::TextIndex;
use crate::shared::error::{RaError, Result};
use crate::shared::ids::{MemoryId, UserId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{
    Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::tokenizer::{
    LowerCaser, RemoveLongFilter, SimpleTokenizer, StopWordFilter, TextAnalyzer,
};
use tantivy::{Index, IndexWriter, TantivyDocument, Term};

/// tantivy wants a generous heap per writer; this is its documented
/// minimum and is ample for memory-sized documents.
const WRITER_HEAP_BYTES: usize = 15_000_000;

/// Name of this adapter's analyzer, registered on every opened index
/// and referenced by the schema's text fields.
const TEXT_ANALYZER: &str = "ra_text_v2";

/// The on-disk layout version, its own subdirectory under the index
/// root. Bump it (and rename the analyzer) whenever the schema or the
/// analyzer changes, so existing stores are ignored rather than
/// silently misbehaving; the rebuilder then repopulates the new one
/// from the `memories` table.
///
/// Bumping this is safe: the index is derived state, rebuilt on first
/// start (see the rebuilder use case), and never read back into
/// anything but keyword recall.
pub const TEXT_INDEX_VERSION: &str = "v2";

/// The indexing options every tokenised text field shares: the custom
/// analyzer, positions kept (as with the `TEXT` default this replaced).
fn text_options() -> TextOptions {
    TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(TEXT_ANALYZER)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    )
}

/// The analyzer itself: `SimpleTokenizer` → length cap → lowercase →
/// stopword removal. The order matters the way tantivy's own `default`
/// chain orders it; the length cap keeps its documented 40-byte limit
/// so long tokens behave exactly as they did before.
fn text_analyzer() -> TextAnalyzer {
    TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(40))
        .filter(LowerCaser)
        .filter(StopWordFilter::remove(
            STOP_WORDS.iter().map(|word| word.to_string()),
        ))
        .build()
}

struct Fields {
    memory_id: Field,
    content: Field,
    tags: Field,
    category: Field,
}

struct UserIndex {
    index: Index,
    /// `None` on a read-only open — see [`TantivyTextIndex::open_read_only`].
    writer: Option<IndexWriter>,
}

pub struct TantivyTextIndex {
    root: PathBuf,
    schema: Schema,
    fields: Fields,
    /// `false` for the daemon's read-write index. `true` for the audit's,
    /// which must neither write nor take the per-directory writer lock the
    /// daemon is holding (module docs).
    read_only: bool,
    // Opened lazily per user and kept for the process's lifetime.
    indexes: Mutex<HashMap<UserId, Arc<Mutex<UserIndex>>>>,
}

impl TantivyTextIndex {
    pub fn open(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root).map_err(|e| {
            RaError::Internal(format!(
                "failed to create the text index directory {}: {e}",
                root.display()
            ))
        })?;

        Ok(Self::new(root, false))
    }

    /// The same indexes, opened for reading only
    /// (`reranking-plan.md Phase 6.2`).
    ///
    /// Two things this guarantees, both of them the audit's: nothing here
    /// writes a byte to the index, and no `IndexWriter` is created — so the
    /// per-directory writer lock tantivy takes stays free and the command
    /// works against a store a running daemon is serving. Indexes that do
    /// not exist yet are an error rather than an empty leg: a silently dead
    /// keyword leg would misreport the ordering the operator came to read.
    pub fn open_read_only(root: PathBuf) -> Result<Self> {
        Ok(Self::new(root, true))
    }

    fn new(root: PathBuf, read_only: bool) -> Self {
        let mut builder = Schema::builder();
        // STRING (not TEXT) for the id: it must match exactly, never be
        // tokenised.
        let memory_id = builder.add_text_field("memory_id", STRING | STORED);
        // Custom analyzer instead of tantivy's `TEXT`: same tokenising,
        // cap and positions, plus stopword removal (see module docs).
        let content = builder.add_text_field("content", text_options());
        // Tags and category are tokenised too, so `category:decision`
        // style queries and bare tag words both hit.
        let tags = builder.add_text_field("tags", text_options());
        let category = builder.add_text_field("category", text_options());
        let schema = builder.build();

        Self {
            root,
            schema: schema.clone(),
            fields: Fields {
                memory_id,
                tags,
                content,
                category,
            },
            read_only,
            indexes: Mutex::new(HashMap::new()),
        }
    }

    fn user_index(&self, user_id: UserId) -> Result<Arc<Mutex<UserIndex>>> {
        let mut indexes = self
            .indexes
            .lock()
            .map_err(|_| RaError::Internal("text index map poisoned".to_string()))?;

        if let Some(existing) = indexes.get(&user_id) {
            return Ok(Arc::clone(existing));
        }

        let directory = self.user_directory(user_id);
        if self.read_only {
            let index = Index::open_in_dir(&directory).map_err(|e| {
                RaError::Internal(format!(
                    "no keyword index to read at {}: {e}. Start the daemon once, or \
                     stop it and run `recuerdos-ai reindex --text`.",
                    directory.display()
                ))
            })?;
            index.tokenizers().register(TEXT_ANALYZER, text_analyzer());
            let user_index = Arc::new(Mutex::new(UserIndex {
                index,
                writer: None,
            }));
            indexes.insert(user_id, Arc::clone(&user_index));
            return Ok(user_index);
        }

        std::fs::create_dir_all(&directory).map_err(|e| {
            RaError::Internal(format!(
                "failed to create the text index directory {}: {e}",
                directory.display()
            ))
        })?;

        let index = Index::open_in_dir(&directory)
            .or_else(|_| Index::create_in_dir(&directory, self.schema.clone()))
            .map_err(|e| {
                RaError::Internal(format!(
                    "failed to open the text index at {}: {e}",
                    directory.display()
                ))
            })?;
        // Registered per instance: tokenizer managers are not persisted
        // in the index, so every open has to do it or the schema's
        // fields reference an analyzer that isn't there.
        index.tokenizers().register(TEXT_ANALYZER, text_analyzer());

        let writer = index
            .writer(WRITER_HEAP_BYTES)
            .map_err(|e| RaError::Internal(format!("failed to open a text index writer: {e}")))?;

        let user_index = Arc::new(Mutex::new(UserIndex {
            index,
            writer: Some(writer),
        }));
        indexes.insert(user_id, Arc::clone(&user_index));
        Ok(user_index)
    }

    /// Where a user's index of *this version* lives.
    fn user_directory(&self, user_id: UserId) -> PathBuf {
        self.root.join(TEXT_INDEX_VERSION).join(user_id.to_string())
    }

    /// The writer a mutation needs, or an error on a read-only handle —
    /// which never creates one, so a write reaching here means something
    /// was wired to the wrong handle. Failing loudly is the point: silently
    /// dropping an index write would leave a memory unsearchable forever
    /// with nothing but a green result to show for it.
    fn writer_mut(user_index: &mut UserIndex) -> Result<&mut IndexWriter> {
        user_index.writer.as_mut().ok_or_else(|| {
            RaError::Internal("the keyword index is open read-only, so it cannot be written".into())
        })
    }

    /// Removes any existing document for `id`. tantivy has no update:
    /// indexing the same id twice would return it twice.
    fn delete_term(&self, user_index: &mut UserIndex, id: MemoryId) -> Result<()> {
        Self::writer_mut(user_index)?.delete_term(Term::from_field_text(
            self.fields.memory_id,
            &id.to_string(),
        ));
        Ok(())
    }

    /// Deletes any document for `memory` and appends its current text.
    /// Not committed — callers decide when the batch is durable.
    fn reindex_document(&self, user_index: &mut UserIndex, memory: &Memory) -> Result<()> {
        self.delete_term(user_index, memory.id())?;

        let mut document = TantivyDocument::default();
        document.add_text(self.fields.memory_id, memory.id().to_string());
        document.add_text(self.fields.content, memory.content());
        document.add_text(self.fields.tags, memory.tags().join(" "));
        document.add_text(self.fields.category, memory.category().as_str());

        Self::writer_mut(user_index)?
            .add_document(document)
            .map_err(|e| RaError::Internal(format!("failed to index memory text: {e}")))?;
        Ok(())
    }
}

impl TextIndex for TantivyTextIndex {
    fn upsert(&self, context: &UserContext, memory: &Memory) -> Result<()> {
        let user_index = self.user_index(context.user_id())?;
        let mut user_index = user_index
            .lock()
            .map_err(|_| RaError::Internal("text index poisoned".to_string()))?;

        self.reindex_document(&mut user_index, memory)?;

        // Committed synchronously: a memory that isn't immediately
        // recallable looks like a lost write to the agent that just saved
        // it. Batching would be faster, and is the thing to reach for if
        // ingest throughput ever matters more than read-after-write.
        Self::writer_mut(&mut user_index)?
            .commit()
            .map_err(|e| RaError::Internal(format!("failed to commit the text index: {e}")))?;

        Ok(())
    }

    fn upsert_many(&self, context: &UserContext, memories: &[Memory]) -> Result<()> {
        if memories.is_empty() {
            return Ok(());
        }

        let user_index = self.user_index(context.user_id())?;
        let mut user_index = user_index
            .lock()
            .map_err(|_| RaError::Internal("text index poisoned".to_string()))?;

        // No commit until the whole chunk is added: this is what makes a
        // bulk rebuild O(chunks) commits rather than O(memories). If an
        // add fails mid-chunk nothing is committed, so a retry (the same
        // ids, deleted first) overwrites cleanly.
        for memory in memories {
            self.reindex_document(&mut user_index, memory)?;
        }

        Self::writer_mut(&mut user_index)?
            .commit()
            .map_err(|e| RaError::Internal(format!("failed to commit the text index: {e}")))?;

        Ok(())
    }

    fn is_populated(&self, context: &UserContext) -> bool {
        // Directory existence is the signal: it is only ever created
        // alongside a real index of the current version (lazily, on the
        // first write, or by the rebuilder), so a user who saved nothing
        // since the version bump simply has none.
        self.user_directory(context.user_id()).exists()
    }

    fn remove(&self, context: &UserContext, id: MemoryId) -> Result<()> {
        let user_index = self.user_index(context.user_id())?;
        let mut user_index = user_index
            .lock()
            .map_err(|_| RaError::Internal("text index poisoned".to_string()))?;

        self.delete_term(&mut user_index, id)?;
        Self::writer_mut(&mut user_index)?
            .commit()
            .map_err(|e| RaError::Internal(format!("failed to commit the text index: {e}")))?;

        Ok(())
    }

    fn search(
        &self,
        context: &UserContext,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>> {
        if limit == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let user_index = self.user_index(context.user_id())?;
        let user_index = user_index
            .lock()
            .map_err(|_| RaError::Internal("text index poisoned".to_string()))?;

        let reader = user_index
            .index
            .reader()
            .map_err(|e| RaError::Internal(format!("failed to open a text index reader: {e}")))?;
        let searcher = reader.searcher();

        // Left at tantivy's default (OR). Requiring every term would make
        // a natural-language query like "which package manager do I
        // prefer" match nothing; with OR, BM25 ranks by how many and how
        // rare the matched terms are, which is the behaviour wanted here.
        let parser = QueryParser::for_index(
            &user_index.index,
            vec![self.fields.content, self.fields.tags, self.fields.category],
        );

        let parsed = match parser.parse_query(&escape_query(query)) {
            Ok(parsed) => parsed,
            // A query that trips the parser is a user typing punctuation,
            // not a server fault: no keyword hits, and the vector leg
            // still answers.
            Err(_) => return Ok(Vec::new()),
        };

        let hits = searcher
            .search(&parsed, &TopDocs::with_limit(limit))
            .map_err(|e| RaError::Internal(format!("text search failed: {e}")))?;

        let mut hits_by_id = Vec::with_capacity(hits.len());
        for (score, address) in hits {
            let document: TantivyDocument = searcher
                .doc(address)
                .map_err(|e| RaError::Internal(format!("failed to read an indexed doc: {e}")))?;

            if let Some(raw) = document
                .get_first(self.fields.memory_id)
                .and_then(|value| value.as_str())
                && let Ok(id) = MemoryId::from_str(raw)
            {
                hits_by_id.push((id, score));
            }
        }

        Ok(hits_by_id)
    }
}

/// Strips the query-language syntax tantivy would otherwise interpret.
///
/// Callers send natural language ("what's my package manager?"), not
/// queries. A bare `+`, `-` or `:` would change the query's meaning or
/// fail to parse, so the punctuation is dropped rather than honoured.
fn escape_query(query: &str) -> String {
    query
        .chars()
        .map(|c| match c {
            '+' | '-' | '!' | '(' | ')' | '{' | '}' | '[' | ']' | '^' | '"' | '~' | '*' | '?'
            | ':' | '\\' | '/' => ' ',
            other => other,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_removes_query_syntax_but_keeps_words() {
        assert_eq!(
            escape_query("what's my +package -manager?"),
            "what's my package manager"
        );
        assert_eq!(escape_query("category:decision"), "category decision");
        assert_eq!(escape_query("   "), "");
    }

    #[test]
    fn escaping_leaves_ordinary_text_alone() {
        assert_eq!(escape_query("user prefers pnpm"), "user prefers pnpm");
    }

    /// A store with one keyword index, written by a handle that is *still
    /// open* — which is the situation `rerank audit` has to survive: the
    /// daemon's writer holds tantivy's per-directory lock the whole time.
    struct Fixture {
        root: PathBuf,
        writable: TantivyTextIndex,
        alex: UserContext,
        sam: UserContext,
        /// Declared last so it outlives the index handles that live inside it.
        #[allow(dead_code)]
        scratch: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        use crate::bootstrap::wiring::Identity;
        use crate::identity::domain::scope::Scope;

        let scratch = tempfile::tempdir().expect("a scratch directory");
        let root = scratch.path().join("text-index");
        let writable = TantivyTextIndex::open(root.clone()).unwrap();
        let database = Arc::new(crate::shared::sqlite::SqliteDatabase::open_in_memory().unwrap());
        let identity = Identity::from_database(database).unwrap();
        let authenticate = |handle: &str| {
            identity.user_creator.execute(handle, None).unwrap();
            let issued = identity
                .api_key_issuer
                .execute(handle, vec![Scope::Admin], "test")
                .unwrap();
            identity
                .key_authenticator
                .execute(&issued.token.render())
                .unwrap()
        };
        Fixture {
            root,
            writable,
            alex: authenticate("alex"),
            sam: authenticate("sam"),
            scratch,
        }
    }

    fn memory(context: &UserContext, content: &str) -> crate::memories::domain::memory::Memory {
        crate::memories::domain::memory::Memory::create(
            context.user_id(),
            crate::memories::application::test_doubles::new_memory(content),
            crate::memories::application::test_doubles::now(),
        )
        .unwrap()
    }

    #[test]
    fn a_read_only_open_reads_what_the_writer_committed() {
        let fixture = fixture();
        fixture
            .writable
            .upsert(
                &fixture.alex,
                &memory(&fixture.alex, "the user prefers pnpm"),
            )
            .unwrap();

        let readable = TantivyTextIndex::open_read_only(fixture.root.clone()).unwrap();
        let hits = readable.search(&fixture.alex, "pnpm", 5).unwrap();
        assert_eq!(hits.len(), 1, "the read-only leg found nothing");

        // Isolation is structural, not a courtesy: the other user's store is
        // empty, and the one that does not exist yet is an error rather than
        // a silently dead leg.
        assert!(readable.search(&fixture.sam, "pnpm", 5).is_err());
        assert!(!readable.is_populated(&fixture.sam));
        assert!(readable.is_populated(&fixture.alex));
    }

    #[test]
    fn a_read_only_handle_refuses_every_mutation_it_is_handed() {
        let fixture = fixture();
        let kept = memory(&fixture.alex, "the user prefers pnpm");
        fixture.writable.upsert(&fixture.alex, &kept).unwrap();

        let readable = TantivyTextIndex::open_read_only(fixture.root.clone()).unwrap();
        for outcome in [
            readable.upsert(&fixture.alex, &memory(&fixture.alex, "a new fact")),
            readable.upsert_many(
                &fixture.alex,
                &[
                    memory(&fixture.alex, "another fact"),
                    memory(&fixture.alex, "and one more"),
                ],
            ),
            readable.remove(&fixture.alex, kept.id()),
        ] {
            let error = outcome.expect_err("a read-only index must refuse to be written");
            assert!(
                error.to_string().contains("read-only"),
                "the refusal should say why: {error}"
            );
        }

        // Nothing happened: the one committed document is still the only one,
        // and the scratch directory is still there to prove the run left the
        // store as it found it.
        assert_eq!(readable.search(&fixture.alex, "pnpm", 5).unwrap().len(), 1);
        assert!(
            readable
                .search(&fixture.alex, "another", 5)
                .unwrap()
                .is_empty()
        );
        drop(readable);
        assert!(fixture.scratch.path().exists());
    }

    #[test]
    fn a_read_only_open_names_the_repair_when_there_is_nothing_to_read() {
        let fixture = fixture();
        let readable = TantivyTextIndex::open_read_only(fixture.root).unwrap();
        let error = readable
            .search(&fixture.sam, "pnpm", 5)
            .expect_err("no index exists for this user");
        let message = error.to_string();
        assert!(
            message.contains("reindex"),
            "the operator should be told what to run: {message}"
        );
    }
}
