//! Composition root: config → wiring → server start. The only module
//! allowed to see every context's infrastructure and wire concrete
//! implementations into use cases (boundary rule 3).

pub mod calibrate;
pub mod config;
pub mod consolidation_wiring;
pub mod eval;
pub mod memories_wiring;
pub mod rerank_audit;
pub mod server;
pub mod state;
pub mod understanding_wiring;
pub mod wiring;
