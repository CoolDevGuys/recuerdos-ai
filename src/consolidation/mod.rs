//! Consolidation context: background jobs — dedup/merge, decay,
//! distillation, profile digest. Owns `ConsolidationRun`, `Distillation`,
//! `ProfileDigest`.

pub mod application;
pub mod domain;
pub mod infrastructure;
