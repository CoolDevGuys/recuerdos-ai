//! Use cases: one doer per file, each exposing a single public `execute`.

pub mod direct_memory_saver;
pub mod memory_exporter;
pub mod memory_finder;
pub mod memory_forgetter;
pub mod memory_recaller;
pub mod memory_updater;

#[cfg(test)]
pub mod fake_embedder;
#[cfg(test)]
pub mod test_doubles;
