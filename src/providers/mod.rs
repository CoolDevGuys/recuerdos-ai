//! Providers context: concrete LLM/embedding implementations (Anthropic,
//! OpenAI-compat, Ollama, local ONNX) — implementations of traits owned by
//! their consumer contexts (`memories::domain::Embedder`,
//! `understanding::domain::ChatModel`).

// Same island contract as `memories`: wired into the binary in Task 2.6,
// and this allow is removed at the end of Phase 2.
#![allow(dead_code)]

pub mod application;
pub mod domain;
pub mod infrastructure;
