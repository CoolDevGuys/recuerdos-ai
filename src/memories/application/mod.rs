//! Use cases: one doer per file, each exposing a single public `execute`.

pub mod category_curator;
pub mod direct_memory_saver;
pub mod memory_bulk_editor;
pub mod memory_exporter;
pub mod memory_finder;
pub mod memory_forgetter;
pub mod memory_mapper;
pub mod memory_recaller;
pub mod memory_restorer;
pub mod memory_updater;
pub mod profile_assembler;
pub mod structured_memory_saver;
pub mod structured_memory_updater;
pub mod text_index_rebuilder;

#[cfg(test)]
pub mod fake_embedder;
#[cfg(test)]
pub mod test_doubles;
