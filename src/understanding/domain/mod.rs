//! Pure domain: `Candidate`, `Taxonomy`, prompt assembly as pure functions,
//! and the `ChatModel` contract consumed by this context's use cases.

pub mod candidate;
pub mod chat_model;
pub mod extraction_prompt;
pub mod ingest_job;
pub mod ingest_pipeline;
pub mod reconciliation;
pub mod taxonomy;
