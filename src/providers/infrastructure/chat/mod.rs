//! `ChatModel` implementations. The trait is owned by `understanding`
//! (its consumer) — see boundary rule 5.
//!
//! Three transports and one decorator. The transports know a wire format
//! and nothing else; everything they would otherwise duplicate — JSON
//! recovery, error classification, retry policy — lives beside them so
//! that behaviour cannot drift between providers.
//!
//! The scripted test double is deliberately *not* here: it is a test
//! concern of the understanding use cases, so it sits with them rather
//! than masquerading as a provider (the same call made for the fake
//! embedder in `embeddings/`).

pub mod anthropic_chat_model;
#[cfg(test)]
mod contract_tests;
pub mod ollama_chat_model;
pub mod openai_compat_chat_model;
pub mod retrying_chat_model;
pub mod structured_text;
pub mod transport;
