//! Composition root: config → wiring → server start. The only module
//! allowed to see every context's infrastructure and wire concrete
//! implementations into use cases (boundary rule 3).

pub mod config;
pub mod server;
pub mod state;
pub mod wiring;
