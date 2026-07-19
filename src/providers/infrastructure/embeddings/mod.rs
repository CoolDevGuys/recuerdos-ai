//! `Embedder` implementations. The trait is owned by `memories` (its
//! consumer) — see boundary rule 5.

pub mod fake_embedder;
pub mod fastembed_embedder;
