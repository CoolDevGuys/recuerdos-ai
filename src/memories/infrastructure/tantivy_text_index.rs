//! `TextIndex` backed by tantivy — the BM25 leg of hybrid search.
//!
//! # One index per user
//!
//! Each user gets their own directory under `<data>/text-index/<user>/`.
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
use tantivy::schema::{Field, STORED, STRING, Schema, TEXT, Value};
use tantivy::{Index, IndexWriter, TantivyDocument, Term};

/// tantivy wants a generous heap per writer; this is its documented
/// minimum and is ample for memory-sized documents.
const WRITER_HEAP_BYTES: usize = 15_000_000;

struct Fields {
    memory_id: Field,
    content: Field,
    tags: Field,
    category: Field,
}

struct UserIndex {
    index: Index,
    writer: IndexWriter,
}

pub struct TantivyTextIndex {
    root: PathBuf,
    schema: Schema,
    fields: Fields,
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

        let mut builder = Schema::builder();
        // STRING (not TEXT) for the id: it must match exactly, never be
        // tokenised.
        let memory_id = builder.add_text_field("memory_id", STRING | STORED);
        let content = builder.add_text_field("content", TEXT);
        // Tags and category are tokenised too, so `category:decision`
        // style queries and bare tag words both hit.
        let tags = builder.add_text_field("tags", TEXT);
        let category = builder.add_text_field("category", TEXT);
        let schema = builder.build();

        Ok(Self {
            root,
            schema: schema.clone(),
            fields: Fields {
                memory_id,
                content,
                tags,
                category,
            },
            indexes: Mutex::new(HashMap::new()),
        })
    }

    fn user_index(&self, user_id: UserId) -> Result<Arc<Mutex<UserIndex>>> {
        let mut indexes = self
            .indexes
            .lock()
            .map_err(|_| RaError::Internal("text index map poisoned".to_string()))?;

        if let Some(existing) = indexes.get(&user_id) {
            return Ok(Arc::clone(existing));
        }

        let directory = self.root.join(user_id.to_string());
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

        let writer = index
            .writer(WRITER_HEAP_BYTES)
            .map_err(|e| RaError::Internal(format!("failed to open a text index writer: {e}")))?;

        let user_index = Arc::new(Mutex::new(UserIndex { index, writer }));
        indexes.insert(user_id, Arc::clone(&user_index));
        Ok(user_index)
    }

    /// Removes any existing document for `id`. tantivy has no update:
    /// indexing the same id twice would return it twice.
    fn delete_term(&self, user_index: &mut UserIndex, id: MemoryId) {
        user_index.writer.delete_term(Term::from_field_text(
            self.fields.memory_id,
            &id.to_string(),
        ));
    }
}

impl TextIndex for TantivyTextIndex {
    fn upsert(&self, context: &UserContext, memory: &Memory) -> Result<()> {
        let user_index = self.user_index(context.user_id())?;
        let mut user_index = user_index
            .lock()
            .map_err(|_| RaError::Internal("text index poisoned".to_string()))?;

        self.delete_term(&mut user_index, memory.id());

        let mut document = TantivyDocument::default();
        document.add_text(self.fields.memory_id, memory.id().to_string());
        document.add_text(self.fields.content, memory.content());
        document.add_text(self.fields.tags, memory.tags().join(" "));
        document.add_text(self.fields.category, memory.category().as_str());

        user_index
            .writer
            .add_document(document)
            .map_err(|e| RaError::Internal(format!("failed to index memory text: {e}")))?;

        // Committed synchronously: a memory that isn't immediately
        // recallable looks like a lost write to the agent that just saved
        // it. Batching would be faster, and is the thing to reach for if
        // ingest throughput ever matters more than read-after-write.
        user_index
            .writer
            .commit()
            .map_err(|e| RaError::Internal(format!("failed to commit the text index: {e}")))?;

        Ok(())
    }

    fn remove(&self, context: &UserContext, id: MemoryId) -> Result<()> {
        let user_index = self.user_index(context.user_id())?;
        let mut user_index = user_index
            .lock()
            .map_err(|_| RaError::Internal("text index poisoned".to_string()))?;

        self.delete_term(&mut user_index, id);
        user_index
            .writer
            .commit()
            .map_err(|e| RaError::Internal(format!("failed to commit the text index: {e}")))?;

        Ok(())
    }

    fn search(&self, context: &UserContext, query: &str, limit: usize) -> Result<Vec<MemoryId>> {
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

        let mut ids = Vec::with_capacity(hits.len());
        for (_score, address) in hits {
            let document: TantivyDocument = searcher
                .doc(address)
                .map_err(|e| RaError::Internal(format!("failed to read an indexed doc: {e}")))?;

            if let Some(raw) = document
                .get_first(self.fields.memory_id)
                .and_then(|value| value.as_str())
                && let Ok(id) = MemoryId::from_str(raw)
            {
                ids.push(id);
            }
        }

        Ok(ids)
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
}
