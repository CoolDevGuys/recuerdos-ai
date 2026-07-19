//! Pure domain: `Memory` aggregate, value objects, `RecallRanker` scoring
//! policy, and the `MemoryRepository`/`VectorIndex`/`TextIndex`/`Embedder`
//! contracts consumed by this context's use cases.

pub mod category;
pub mod embedder;
pub mod memory;
pub mod memory_repository;
pub mod recall_query;
pub mod recall_ranker;
pub mod text_index;
pub mod vector_index;
