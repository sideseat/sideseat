use thiserror::Error;

use super::types::{ArtifactSetId, BranchId, CanvasId, ConversationId, NodeId, SourceId};

/// All errors returned by [`ContextManager`](super::ContextManager) and related types.
#[derive(Debug, Clone, Error)]
pub enum CmError {
    #[error("Node not found: {0}")]
    NodeNotFound(NodeId),

    #[error("Branch not found: {0}")]
    BranchNotFound(BranchId),

    #[error("Conversation not found: {0}")]
    ConversationNotFound(ConversationId),

    #[error("Canvas not found: {0}")]
    CanvasNotFound(CanvasId),

    #[error("Artifact not found: {0}")]
    ArtifactNotFound(ArtifactSetId),

    #[error("Node is finalized: {0}")]
    NodeFinalized(NodeId),

    #[error("Backend error: {0}")]
    BackendError(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("CRDT error: {0}")]
    Crdt(String),

    #[error("Version mismatch for {id}: expected {expected}, got {actual}")]
    VersionMismatch {
        id: String,
        expected: u64,
        actual: u64,
    },

    #[error("Unsupported schema version: found {found}, max supported {max}")]
    UnsupportedVersion { found: u32, max: u32 },

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Cycle detected: {0}")]
    CycleDetected(String),

    #[error("Context overflow: {0}")]
    ContextOverflow(String),

    #[error("Source not found: {0}")]
    SourceNotFound(SourceId),

    #[error("Extraction failed: {0}")]
    ExtractionFailed(String),

    #[error("Filesystem error: {0}")]
    FsError(String),

    #[error("VFS extension is not registered on this ContextManager instance")]
    VfsNotConfigured,

    #[error("No nodes in conversation — add at least one node before spawning an agent")]
    NoNodes,
}

impl From<serde_json::Error> for CmError {
    fn from(e: serde_json::Error) -> Self {
        CmError::Serialization(e.to_string())
    }
}
