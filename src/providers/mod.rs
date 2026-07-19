//! Providers context: concrete LLM/embedding implementations (Anthropic,
//! OpenAI-compat, Ollama, local ONNX) — implementations of traits owned by
//! their consumer contexts (`memories::domain::Embedder`,
//! `understanding::domain::ChatModel`).

pub mod application;
pub mod domain;
pub mod infrastructure;
