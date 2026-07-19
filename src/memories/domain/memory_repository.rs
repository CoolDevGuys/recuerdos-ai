//! Storage contract for memories.
//!
//! Every method takes `&UserContext` rather than a `UserId`. That is the
//! isolation guarantee in mechanical form: a caller cannot ask for
//! another user's rows without first authenticating as them, because it
//! cannot manufacture the context (see
//! `identity/domain/user_context.rs`).

use super::memory::Memory;
use crate::identity::domain::user_context::UserContext;
use crate::shared::error::Result;
use crate::shared::ids::MemoryId;
use chrono::{DateTime, Utc};

/// A record of every mutation, for the audit trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOperation {
    Add,
    Update,
    Delete,
    Supersede,
}

impl AuditOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditOperation::Add => "add",
            AuditOperation::Update => "update",
            AuditOperation::Delete => "delete",
            AuditOperation::Supersede => "supersede",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub memory_id: MemoryId,
    pub operation: AuditOperation,
    /// Which client or use case did it.
    pub actor: String,
    /// Human-readable summary of what changed.
    pub detail: String,
    pub at: DateTime<Utc>,
}

pub trait MemoryRepository: Send + Sync {
    fn insert(&self, context: &UserContext, memory: &Memory, actor: &str) -> Result<()>;

    fn update(&self, context: &UserContext, memory: &Memory, actor: &str) -> Result<()>;

    /// Soft delete: the row is retained and the audit entry written, but
    /// the memory stops appearing in recall. Hard deletion belongs to the
    /// governance path (project-plan.md §15), not to ordinary use.
    fn delete(&self, context: &UserContext, id: MemoryId, actor: &str) -> Result<()>;

    fn find(&self, context: &UserContext, id: MemoryId) -> Result<Option<Memory>>;

    /// Fetches many by id, in one round trip. Ids belonging to another
    /// user are simply absent from the result.
    fn find_many(&self, context: &UserContext, ids: &[MemoryId]) -> Result<Vec<Memory>>;

    /// All of this user's memories, newest first, for export.
    fn list(&self, context: &UserContext, include_inactive: bool) -> Result<Vec<Memory>>;

    fn audit_trail(&self, context: &UserContext, limit: usize) -> Result<Vec<AuditEntry>>;

    /// Records that these memories were returned by a recall. Off the
    /// hot path; feeds Phase 5's importance decay.
    fn touch_accessed(
        &self,
        context: &UserContext,
        ids: &[MemoryId],
        now: DateTime<Utc>,
    ) -> Result<()>;
}
