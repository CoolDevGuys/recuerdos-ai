//! Model-backed reranking adapters.
//!
//! Lives next to `embeddings` because it is the same kind of thing: local
//! ONNX inference with a HuggingFace download behind it, so the same
//! conventions apply — the cache directory is always set explicitly
//! (`[embeddings].cache_dir`), the download happens at startup or at
//! image-build time (`warm-models`), never lazily on a request path that
//! promised an answer. The contract it implements is owned by
//! `memories::domain::reranker`.

pub mod fastembed_reranker;
