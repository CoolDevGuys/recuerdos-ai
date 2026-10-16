//! Concrete adapters: `FastembedEmbedder`, `AnthropicChatModel`,
//! `OpenAiCompatChatModel`, `OllamaChatModel`, `FastembedReranker` —
//! technology-prefixed implementations of traits owned by
//! `memories`/`understanding`.

pub mod chat;
pub mod embeddings;
pub mod rerank;
