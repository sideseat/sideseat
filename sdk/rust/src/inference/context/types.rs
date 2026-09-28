use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{ContentBlock, StopReason, Usage};

// ---------------------------------------------------------------------------
// Newtype IDs — UUIDv7 (time-sortable)
// ---------------------------------------------------------------------------

/// Defines a strongly-typed newtype ID backed by a UUIDv7 string.
///
/// Generated types implement `new()`, `from_string()`, `as_str()`, `Default`, `Display`,
/// and `Deref<Target = str>`. IDs are time-sortable (UUIDv7) and cheaply cloneable.
#[macro_export]
macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::now_v7().to_string())
            }

            pub fn from_string(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }
    };
}

define_id!(NodeId);
define_id!(ConversationId);
define_id!(BranchId);
define_id!(UserId);
define_id!(CanvasId);
define_id!(ArtifactSetId);
define_id!(SourceId);
define_id!(AgentId);
define_id!(KanbanBoardId);
define_id!(PromptId);
define_id!(DatasetEntryId);

// ---------------------------------------------------------------------------
// Time helper
// ---------------------------------------------------------------------------

/// Current Unix timestamp in microseconds.
pub fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as i64
}

// ---------------------------------------------------------------------------
// ConversationStatus
// ---------------------------------------------------------------------------

/// Lifecycle state of a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConversationStatus {
    #[default]
    Active,
    Archived,
    Deleted,
}

// ---------------------------------------------------------------------------
// MaybeCleared — three-state wrapper for optional fields in patches
// ---------------------------------------------------------------------------

/// Three-state wrapper for fields that can be explicitly cleared (set to None).
/// Avoids ambiguity of `Option<Option<T>>` in serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", content = "v")]
pub enum MaybeCleared<T> {
    Set(T),
    Clear,
}

// ---------------------------------------------------------------------------
// ConversationPatch — append-only granular update
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConversationPatch {
    pub title: Option<String>,
    pub status: Option<ConversationStatus>,
    pub instructions: Option<MaybeCleared<String>>,
    pub metadata: Option<HashMap<String, Value>>,
}

// ---------------------------------------------------------------------------
// Conversation
// ---------------------------------------------------------------------------

/// Root entity for a multi-turn context session.
///
/// Owns one or more branches (default `"main"`), an embedded [`Workspace`],
/// and optional system-level instructions for agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: ConversationId,
    pub title: Option<String>,
    pub icon: Option<ConversationIcon>,
    pub created_at: i64,
    pub updated_at: i64,
    pub created_by: Option<UserId>,
    pub default_model: Option<String>,
    pub default_provider: Option<String>,
    pub mode: ConversationMode,
    pub status: ConversationStatus,
    pub workspace: Workspace,
    pub instructions: Option<String>,
    pub default_branch_id: Option<BranchId>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
    #[serde(default)]
    pub title_generated: bool,
    #[serde(default)]
    pub icon_generated: bool,
}

impl Conversation {
    pub fn new(title: impl Into<String>) -> Self {
        let now = now_micros();
        Self {
            id: ConversationId::new(),
            title: Some(title.into()),
            icon: None,
            created_at: now,
            updated_at: now,
            created_by: None,
            default_model: None,
            default_provider: None,
            mode: ConversationMode::default(),
            status: ConversationStatus::default(),
            workspace: Workspace::default(),
            instructions: None,
            default_branch_id: None,
            metadata: HashMap::new(),
            title_generated: false,
            icon_generated: false,
        }
    }

    pub fn apply_patch(&mut self, patch: &ConversationPatch) {
        if let Some(title) = &patch.title {
            self.title = Some(title.clone());
        }
        if let Some(status) = &patch.status {
            self.status = status.clone();
        }
        if let Some(instr) = &patch.instructions {
            match instr {
                MaybeCleared::Set(s) => self.instructions = Some(s.clone()),
                MaybeCleared::Clear => self.instructions = None,
            }
        }
        if let Some(meta) = &patch.metadata {
            self.metadata.extend(meta.clone());
        }
        self.updated_at = now_micros();
    }
}

// ---------------------------------------------------------------------------
// Workspace (embedded in Conversation)
// ---------------------------------------------------------------------------

/// Shared workspace resources associated with a conversation.
///
/// Holds typed references to all persistent tools (canvases, kanban boards,
/// artifact sets, sources, agents, skills) accessible on any branch.
/// All collection fields are `#[serde(default)]` for forward compatibility.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(default)]
    pub canvases: Vec<CanvasId>,
    #[serde(default)]
    pub artifact_sets: Vec<ArtifactSetId>,
    #[serde(default)]
    pub kanban_ids: Vec<KanbanBoardId>,
    pub plan_id: Option<String>,
    #[serde(default)]
    pub skills: Vec<SkillRef>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerRef>,
    #[serde(default)]
    pub a2a_agents: Vec<A2aAgentRef>,
    #[serde(default)]
    pub knowledge_bases: Vec<KnowledgeBaseRef>,
    #[serde(default)]
    pub sources: Vec<SourceId>,
}

/// Reference to a registered skill usable within this workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRef {
    pub skill_id: String,
    pub name: String,
    #[serde(default)]
    pub config: HashMap<String, Value>,
}

/// Reference to an MCP server exposed to agents in this workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerRef {
    pub server_id: String,
    pub label: String,
    pub transport: String,
    pub url: Option<String>,
    #[serde(default)]
    pub enabled_tools: Vec<String>,
}

/// Reference to an A2A (agent-to-agent) peer callable from this workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aAgentRef {
    pub agent_id: String,
    pub name: String,
    pub endpoint: String,
    pub agent_card: Option<Value>,
}

/// Reference to an external knowledge base retrievable from this workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeBaseRef {
    pub kb_id: String,
    pub name: String,
    pub kb_type: String,
    pub endpoint: Option<String>,
    #[serde(default)]
    pub config: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

/// Immutable append-only record of one interaction turn.
///
/// Nodes form a parent-linked tree: `parent_id` points to the preceding node on the
/// same branch (or the fork point when crossing branches). `sequence` is monotonically
/// increasing per branch. `version` tracks streaming updates — the latest content is
/// always in the highest-version row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub conversation_id: ConversationId,
    pub branch_id: BranchId,
    pub parent_id: Option<NodeId>,
    pub sequence: u64,
    pub created_at: i64,
    pub created_by: Option<UserId>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub content: NodeContent,
    pub usage: Option<Usage>,
    pub version: u64,
    pub is_final: bool,
    pub streaming: Option<StreamingState>,
    pub deleted: bool,
    pub agent_id: Option<AgentId>,
    pub correlation_id: Option<String>,
    pub reply_to: Option<NodeId>,
    #[serde(default)]
    pub eval_scores: Vec<EvalScore>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
    /// CRDT log position at commit time; used to restore CRDT state at a historical node.
    pub crdt_seq_watermark: Option<u64>,
}

impl Node {
    pub fn content_type(&self) -> &'static str {
        self.content.content_type_str()
    }
}

// ---------------------------------------------------------------------------
// NodeHeader — lightweight tree index
// ---------------------------------------------------------------------------

/// Lightweight projection of [`Node`] used for tree index reconstruction.
///
/// Loaded in bulk by [`ContextBackend::list_node_headers`] to avoid fetching full content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHeader {
    pub id: NodeId,
    pub parent_id: Option<NodeId>,
    pub branch_id: BranchId,
    pub sequence: u64,
    pub created_at: i64,
    pub content_type: String,
    pub model: Option<String>,
    pub created_by: Option<UserId>,
    pub is_final: bool,
    pub deleted: bool,
    /// CRDT log position at commit time; required for fork watermark lookup without full node fetch.
    pub crdt_seq_watermark: Option<u64>,
}

impl From<&Node> for NodeHeader {
    fn from(node: &Node) -> Self {
        Self {
            id: node.id.clone(),
            parent_id: node.parent_id.clone(),
            branch_id: node.branch_id.clone(),
            sequence: node.sequence,
            created_at: node.created_at,
            content_type: node.content_type().to_string(),
            model: node.model.clone(),
            created_by: node.created_by.clone(),
            is_final: node.is_final,
            deleted: node.deleted,
            crdt_seq_watermark: node.crdt_seq_watermark,
        }
    }
}

mod node_content;
pub use node_content::{NodeContent, VfsOperation};

// ---------------------------------------------------------------------------
// NodeParams — builder for appending
// ---------------------------------------------------------------------------

/// Optional metadata supplied by the caller when appending a node.
/// All fields default to `None` / empty.
#[derive(Debug, Clone, Default)]
pub struct NodeParams {
    pub created_by: Option<UserId>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub usage: Option<Usage>,
    pub metadata: HashMap<String, Value>,
    pub agent_id: Option<AgentId>,
    pub correlation_id: Option<String>,
    pub reply_to: Option<NodeId>,
}

// ---------------------------------------------------------------------------
// Supporting enums
// ---------------------------------------------------------------------------

/// Operating mode that controls which tools and context strategies are active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConversationMode {
    #[default]
    Chat,
    DeepResearch,
    ComputerUse,
    Agentic,
    CodeGen,
    Custom(String),
}

/// Integration protocol used by a spawned sub-agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    Mcp,
    A2a,
    Skill,
    Custom(String),
}

/// Source of a captured media stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaStreamType {
    ScreenRecording,
    Camera,
    Voice,
    NovaSonic,
}

/// Execution state of an async task or skill invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed { error: String },
    Cancelled,
}

/// Semantic kind of a node annotation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationType {
    Comment,
    Highlight,
    Correction,
    Bookmark,
}

/// Storage system where a binary artifact is physically located.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackend {
    Vfs,
    Local,
    S3,
    Gcs,
    AzureBlob,
    Inline,
    Unknown(String),
}

/// Visual icon displayed alongside a conversation in UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConversationIcon {
    Emoji { value: String },
    Svg { data: String },
    Image { storage_ref: StorageRef },
    Color { value: String },
}

/// Pointer to a stored binary artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageRef {
    pub backend: StorageBackend,
    pub uri: String,
    pub checksum: Option<String>,
    pub size_bytes: Option<u64>,
}

/// Live progress metadata for an in-flight streaming node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingState {
    pub started_at: i64,
    pub tokens_so_far: u64,
    pub last_chunk_at: i64,
    pub status: StreamStatus,
}

/// Lifecycle state of a streaming node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamStatus {
    Active,
    Paused,
    Error { message: String },
    Cancelled,
}

/// A single human interaction with an MCP UI component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiInteraction {
    pub action: String,
    pub data: Value,
    pub user_id: Option<UserId>,
    pub timestamp: i64,
}

// ---------------------------------------------------------------------------
// BranchMeta
// ---------------------------------------------------------------------------

/// Persistent metadata for a conversation branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchMeta {
    pub id: BranchId,
    pub conversation_id: ConversationId,
    pub parent_id: Option<BranchId>,
    pub fork_node_id: Option<NodeId>,
    /// CRDT log position at branch creation (0 for main). Stored persistently so
    /// `fork(branch_b, at_node_n)` can reconstruct CRDT from backend without the node in memory.
    pub crdt_seq_watermark: u64,
    pub name: String,
    pub created_at: i64,
}

// ---------------------------------------------------------------------------
// Reactions
// ---------------------------------------------------------------------------

/// User reaction attached to a node (thumb-up, star, flag, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reaction {
    pub node_id: NodeId,
    pub user_id: UserId,
    pub reaction_type: ReactionType,
    pub created_at: i64,
    pub comment: Option<String>,
}

/// Variety of reaction a user can attach to a node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReactionType {
    ThumbsUp,
    ThumbsDown,
    Star,
    Flag,
    Custom(String),
}

// ---------------------------------------------------------------------------
// EvalScore
// ---------------------------------------------------------------------------

/// Single grader score attached to a node during evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalScore {
    pub name: String,
    pub score: f64,
    pub rationale: Option<String>,
    pub grader: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// PromptVersion
// ---------------------------------------------------------------------------

/// Versioned snapshot of a named prompt template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptVersion {
    pub id: PromptId,
    pub name: String,
    pub content: Vec<crate::types::ContentBlock>,
    pub version: u32,
    pub created_at: i64,
    pub created_by: Option<UserId>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// Dataset types
// ---------------------------------------------------------------------------

/// Dataset partition for train/test/eval splits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetSplit {
    Train,
    Test,
    Eval,
}

/// Labeled input/output pair extracted from a conversation for eval datasets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetEntry {
    pub id: DatasetEntryId,
    pub conversation_id: ConversationId,
    pub dataset_name: String,
    pub input_node_ids: Vec<NodeId>,
    pub output_node_id: NodeId,
    pub expected_output: Option<String>,
    pub split: DatasetSplit,
    pub created_at: i64,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// MemoryEntry types (scope-agnostic: user, conv, or agent)
// ---------------------------------------------------------------------------

define_id!(MemoryEntryId);

/// Semantic category of a [`MemoryEntry`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEntryType {
    Fact,
    Preference,
    Goal,
    Skill,
    Custom(String),
}

/// Persistent memory item scoped to a user, conversation, or agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: MemoryEntryId,
    pub scope_id: String,
    pub content: String,
    pub memory_type: MemoryEntryType,
    pub source_conversation_id: Option<ConversationId>,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
