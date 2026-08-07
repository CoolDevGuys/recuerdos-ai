//! Pure domain: `ClusterBuilder` (union-find over similarity pairs),
//! decay/expiry math, `ProfileDigest` value objects.

pub mod cluster_builder;
pub mod consolidation_run;
pub mod consolidation_state;
pub mod decay;
pub mod digest_prompt;
pub mod distillation;
pub mod merge_prompt;
pub mod profile_digest;
pub mod similarity;
