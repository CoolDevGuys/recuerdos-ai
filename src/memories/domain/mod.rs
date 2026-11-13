//! Pure domain: `Memory` aggregate, value objects, `RecallRanker` scoring
//! policy, and the `MemoryRepository`/`VectorIndex`/`TextIndex`/`Embedder`/
//! `Reranker` contracts consumed by this context's use cases.

pub mod applies_to;
pub mod category;
pub mod category_catalog;
pub mod conflict_judge;
pub mod content_checks;
pub mod document_text;
pub mod embedder;
pub mod entity_graph;
pub mod entity_key;
pub mod graph_backfill_state;
pub mod kind;
pub mod legacy_structure;
pub mod memory;
pub mod memory_change_observer;
pub mod memory_map;
pub mod memory_repository;
pub mod memory_structure;
pub mod project_registry;
pub mod recall_query;
pub mod recall_ranker;
pub mod reranker;
pub mod text_index;
pub mod topic;
pub mod topic_catalog;
pub mod vector_index;
