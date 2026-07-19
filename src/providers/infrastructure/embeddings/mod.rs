//! `Embedder` implementations. The trait is owned by `memories` (its
//! consumer) — see boundary rule 5.
//!
//! Only real technology lives here. The deterministic test double is a
//! test concern of the `memories` use cases, so it sits with their other
//! doubles rather than masquerading as a provider.

pub mod fastembed_embedder;
