//! Use cases: `CandidateExtractor`, `MemoryReconciler`, `SessionDistiller`.

pub mod candidate_extractor;
pub mod catalog_taxonomy;
pub mod intent_first_ingestor;
pub mod memory_ingestor;
pub mod memory_reconciler;
pub mod model_conflict_judge;
#[cfg(test)]
pub mod scripted_chat_model;
pub mod structure_suggester;
pub mod verbatim_ingestor;
