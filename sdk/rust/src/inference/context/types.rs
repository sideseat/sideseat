use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{ContentBlock, StopReason, Usage};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const INLINE_SIZE_LIMIT: usize = 1024 * 1024; // 1MB

// ---------------------------------------------------------------------------
// Newtype IDs — UUIDv7 (time-sortable)
// ---------------------------------------------------------------------------

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

pub fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as i64
}

// ---------------------------------------------------------------------------
// ConversationStatus
// ---------------------------------------------------------------------------

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(default)]
    pub canvases: Vec<CanvasId>,
    #[serde(default)]
    pub artifact_sets: Vec<ArtifactSetId>,
    pub kanban_id: Option<KanbanBoardId>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRef {
    pub skill_id: String,
    pub name: String,
    #[serde(default)]
    pub config: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerRef {
    pub server_id: String,
    pub label: String,
    pub transport: String,
    pub url: Option<String>,
    #[serde(default)]
    pub enabled_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aAgentRef {
    pub agent_id: String,
    pub name: String,
    pub endpoint: String,
    pub agent_card: Option<Value>,
}

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

// ---------------------------------------------------------------------------
// NodeContent — comprehensive tagged enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeContent {
    // Core conversation
    UserMessage {
        content: Vec<ContentBlock>,
        name: Option<String>,
    },
    AssistantMessage {
        content: Vec<ContentBlock>,
        stop_reason: Option<StopReason>,
        variant_index: Option<u32>,
    },
    SystemMessage {
        content: Vec<ContentBlock>,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<ContentBlock>,
        is_error: bool,
        duration_ms: Option<u64>,
    },

    // Agent orchestration
    AgentSpawn {
        agent_id: String,
        agent_name: String,
        sub_branch_id: BranchId,
        agent_type: AgentType,
        framework: Option<String>,
        a2a_endpoint: Option<String>,
        skill_id: Option<String>,
        is_async: bool,
    },
    AgentResult {
        agent_id: String,
        content: Vec<ContentBlock>,
        usage_total: Option<Usage>,
        was_async: bool,
    },
    AgentEvent {
        agent_id: String,
        event_name: String,
        event_data: Value,
    },
    AgentHandoff {
        from_agent_id: String,
        to_agent_id: String,
        to_agent_name: String,
        reason: Option<String>,
        target_branch_id: BranchId,
    },

    // Media & files
    FileUpload {
        file_id: String,
        filename: String,
        mime_type: String,
        size_bytes: u64,
        storage_ref: StorageRef,
    },
    MediaCapture {
        stream_type: MediaStreamType,
        started_at: i64,
        duration_ms: u64,
        storage_ref: Option<StorageRef>,
        transcription: Option<String>,
    },

    // Workspace references
    ArtifactRef {
        artifact_set_id: ArtifactSetId,
        version: u32,
        summary: Option<String>,
    },
    CanvasRef {
        canvas_id: CanvasId,
        snapshot_version: Option<u64>,
        changed_items: Vec<String>,
    },

    // Interactive
    McpUi {
        component_type: String,
        component_data: Value,
        interactions: Vec<UiInteraction>,
    },
    SkillInvocation {
        skill_id: String,
        skill_name: String,
        input: Value,
        output: Option<Value>,
        status: TaskStatus,
    },
    AsyncTask {
        task_id: String,
        task_type: String,
        description: String,
        status: TaskStatus,
        result: Option<Value>,
    },
    ComputerAction {
        action: Value,
        before_screenshot: Option<StorageRef>,
        after_screenshot: Option<StorageRef>,
        result: Option<Value>,
    },

    // Meta
    ModeSwitch {
        from_mode: Option<ConversationMode>,
        to_mode: ConversationMode,
    },
    Annotation {
        target_node_id: NodeId,
        text: String,
        annotation_type: AnnotationType,
    },

    // Sources
    SourceRef {
        source_id: SourceId,
        source_name: String,
        mime_type: String,
        summary: Option<String>,
    },

    // Eval
    EvalResult {
        target_node_id: NodeId,
        eval_name: String,
        scores: Vec<EvalScore>,
        grader_model: Option<String>,
    },

    // Human-in-the-loop
    ApprovalRequest {
        question: String,
        options: Vec<String>,
        timeout_ms: Option<u64>,
        context: Option<Value>,
    },
    ApprovalResponse {
        request_node_id: NodeId,
        selected: String,
        comment: Option<String>,
        responded_by: Option<UserId>,
    },

    // Workflow
    WorkflowStep {
        step_name: String,
        step_index: u32,
        total_steps: Option<u32>,
        status: TaskStatus,
        inputs: Value,
        outputs: Option<Value>,
        workflow_id: Option<String>,
    },

    // VFS file system changes
    VfsChange {
        path: String,
        operation: VfsOperation,
    },

    // Forward compat
    Unknown {
        kind: String,
        data: Value,
    },
}

impl NodeContent {
    pub fn content_type_str(&self) -> &'static str {
        match self {
            NodeContent::UserMessage { .. } => "user_message",
            NodeContent::AssistantMessage { .. } => "assistant_message",
            NodeContent::SystemMessage { .. } => "system_message",
            NodeContent::ToolResult { .. } => "tool_result",
            NodeContent::AgentSpawn { .. } => "agent_spawn",
            NodeContent::AgentResult { .. } => "agent_result",
            NodeContent::AgentEvent { .. } => "agent_event",
            NodeContent::AgentHandoff { .. } => "agent_handoff",
            NodeContent::FileUpload { .. } => "file_upload",
            NodeContent::MediaCapture { .. } => "media_capture",
            NodeContent::ArtifactRef { .. } => "artifact_ref",
            NodeContent::CanvasRef { .. } => "canvas_ref",
            NodeContent::McpUi { .. } => "mcp_ui",
            NodeContent::SkillInvocation { .. } => "skill_invocation",
            NodeContent::AsyncTask { .. } => "async_task",
            NodeContent::ComputerAction { .. } => "computer_action",
            NodeContent::ModeSwitch { .. } => "mode_switch",
            NodeContent::Annotation { .. } => "annotation",
            NodeContent::SourceRef { .. } => "source_ref",
            NodeContent::EvalResult { .. } => "eval_result",
            NodeContent::ApprovalRequest { .. } => "approval_request",
            NodeContent::ApprovalResponse { .. } => "approval_response",
            NodeContent::WorkflowStep { .. } => "workflow_step",
            NodeContent::VfsChange { .. } => "vfs_change",
            NodeContent::Unknown { .. } => "unknown",
        }
    }
}

/// The kind of change recorded in a [`NodeContent::VfsChange`] node.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum VfsOperation {
    Create { mime_type: String, size_bytes: u64 },
    Modify { size_bytes: u64 },
    Delete,
    Rename { from: String },
    CrdtUpdate { size_bytes: u64 },
}

// Custom Deserialize for forward compat: unknown "type" → Unknown variant
impl<'de> Deserialize<'de> for NodeContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        // Owned so the borrow on `value` is released, allowing `value` to be
        // moved into the Unknown fast-path without a clone.
        let type_str = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned();

        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        enum Inner {
            UserMessage {
                content: Vec<ContentBlock>,
                name: Option<String>,
            },
            AssistantMessage {
                content: Vec<ContentBlock>,
                stop_reason: Option<StopReason>,
                variant_index: Option<u32>,
            },
            SystemMessage {
                content: Vec<ContentBlock>,
            },
            ToolResult {
                tool_use_id: String,
                content: Vec<ContentBlock>,
                is_error: bool,
                duration_ms: Option<u64>,
            },
            AgentSpawn {
                agent_id: String,
                agent_name: String,
                sub_branch_id: BranchId,
                agent_type: AgentType,
                framework: Option<String>,
                a2a_endpoint: Option<String>,
                skill_id: Option<String>,
                is_async: bool,
            },
            AgentResult {
                agent_id: String,
                content: Vec<ContentBlock>,
                usage_total: Option<Usage>,
                was_async: bool,
            },
            AgentEvent {
                agent_id: String,
                event_name: String,
                event_data: Value,
            },
            AgentHandoff {
                from_agent_id: String,
                to_agent_id: String,
                to_agent_name: String,
                reason: Option<String>,
                target_branch_id: BranchId,
            },
            FileUpload {
                file_id: String,
                filename: String,
                mime_type: String,
                size_bytes: u64,
                storage_ref: StorageRef,
            },
            MediaCapture {
                stream_type: MediaStreamType,
                started_at: i64,
                duration_ms: u64,
                storage_ref: Option<StorageRef>,
                transcription: Option<String>,
            },
            ArtifactRef {
                artifact_set_id: ArtifactSetId,
                version: u32,
                summary: Option<String>,
            },
            CanvasRef {
                canvas_id: CanvasId,
                snapshot_version: Option<u64>,
                changed_items: Vec<String>,
            },
            McpUi {
                component_type: String,
                component_data: Value,
                interactions: Vec<UiInteraction>,
            },
            SkillInvocation {
                skill_id: String,
                skill_name: String,
                input: Value,
                output: Option<Value>,
                status: TaskStatus,
            },
            AsyncTask {
                task_id: String,
                task_type: String,
                description: String,
                status: TaskStatus,
                result: Option<Value>,
            },
            ComputerAction {
                action: Value,
                before_screenshot: Option<StorageRef>,
                after_screenshot: Option<StorageRef>,
                result: Option<Value>,
            },
            ModeSwitch {
                from_mode: Option<ConversationMode>,
                to_mode: ConversationMode,
            },
            Annotation {
                target_node_id: NodeId,
                text: String,
                annotation_type: AnnotationType,
            },
            SourceRef {
                source_id: SourceId,
                source_name: String,
                mime_type: String,
                summary: Option<String>,
            },
            EvalResult {
                target_node_id: NodeId,
                eval_name: String,
                scores: Vec<EvalScore>,
                grader_model: Option<String>,
            },
            ApprovalRequest {
                question: String,
                options: Vec<String>,
                timeout_ms: Option<u64>,
                context: Option<Value>,
            },
            ApprovalResponse {
                request_node_id: NodeId,
                selected: String,
                comment: Option<String>,
                responded_by: Option<UserId>,
            },
            WorkflowStep {
                step_name: String,
                step_index: u32,
                total_steps: Option<u32>,
                status: TaskStatus,
                inputs: Value,
                outputs: Option<Value>,
                workflow_id: Option<String>,
            },
            VfsChange {
                path: String,
                operation: VfsOperation,
            },
        }

        const KNOWN_TYPES: &[&str] = &[
            "user_message", "assistant_message", "system_message", "tool_result",
            "agent_spawn", "agent_result", "agent_event", "agent_handoff",
            "file_upload", "media_capture", "artifact_ref", "canvas_ref",
            "mcp_ui", "skill_invocation", "async_task", "computer_action",
            "mode_switch", "annotation", "source_ref", "eval_result",
            "approval_request", "approval_response", "workflow_step", "vfs_change",
        ];

        // Fast path: skip deserialization entirely for types not in this schema version.
        // This avoids cloning `value` for forward-compatibility unknown types.
        if !KNOWN_TYPES.contains(&type_str.as_str()) {
            return Ok(NodeContent::Unknown { kind: type_str, data: value });
        }

        // Clone is unavoidable: `from_value` consumes `value`, but we need
        // it in the Err arm for graceful unknown fallback on schema mismatch.
        match serde_json::from_value::<Inner>(value.clone()) {
            Ok(inner) => Ok(match inner {
                Inner::UserMessage { content, name } => NodeContent::UserMessage { content, name },
                Inner::AssistantMessage { content, stop_reason, variant_index } => {
                    NodeContent::AssistantMessage { content, stop_reason, variant_index }
                }
                Inner::SystemMessage { content } => NodeContent::SystemMessage { content },
                Inner::ToolResult { tool_use_id, content, is_error, duration_ms } => {
                    NodeContent::ToolResult { tool_use_id, content, is_error, duration_ms }
                }
                Inner::AgentSpawn { agent_id, agent_name, sub_branch_id, agent_type, framework, a2a_endpoint, skill_id, is_async } => {
                    NodeContent::AgentSpawn { agent_id, agent_name, sub_branch_id, agent_type, framework, a2a_endpoint, skill_id, is_async }
                }
                Inner::AgentResult { agent_id, content, usage_total, was_async } => {
                    NodeContent::AgentResult { agent_id, content, usage_total, was_async }
                }
                Inner::AgentEvent { agent_id, event_name, event_data } => {
                    NodeContent::AgentEvent { agent_id, event_name, event_data }
                }
                Inner::AgentHandoff { from_agent_id, to_agent_id, to_agent_name, reason, target_branch_id } => {
                    NodeContent::AgentHandoff { from_agent_id, to_agent_id, to_agent_name, reason, target_branch_id }
                }
                Inner::FileUpload { file_id, filename, mime_type, size_bytes, storage_ref } => {
                    NodeContent::FileUpload { file_id, filename, mime_type, size_bytes, storage_ref }
                }
                Inner::MediaCapture { stream_type, started_at, duration_ms, storage_ref, transcription } => {
                    NodeContent::MediaCapture { stream_type, started_at, duration_ms, storage_ref, transcription }
                }
                Inner::ArtifactRef { artifact_set_id, version, summary } => {
                    NodeContent::ArtifactRef { artifact_set_id, version, summary }
                }
                Inner::CanvasRef { canvas_id, snapshot_version, changed_items } => {
                    NodeContent::CanvasRef { canvas_id, snapshot_version, changed_items }
                }
                Inner::McpUi { component_type, component_data, interactions } => {
                    NodeContent::McpUi { component_type, component_data, interactions }
                }
                Inner::SkillInvocation { skill_id, skill_name, input, output, status } => {
                    NodeContent::SkillInvocation { skill_id, skill_name, input, output, status }
                }
                Inner::AsyncTask { task_id, task_type, description, status, result } => {
                    NodeContent::AsyncTask { task_id, task_type, description, status, result }
                }
                Inner::ComputerAction { action, before_screenshot, after_screenshot, result } => {
                    NodeContent::ComputerAction { action, before_screenshot, after_screenshot, result }
                }
                Inner::ModeSwitch { from_mode, to_mode } => {
                    NodeContent::ModeSwitch { from_mode, to_mode }
                }
                Inner::Annotation { target_node_id, text, annotation_type } => {
                    NodeContent::Annotation { target_node_id, text, annotation_type }
                }
                Inner::SourceRef { source_id, source_name, mime_type, summary } => {
                    NodeContent::SourceRef { source_id, source_name, mime_type, summary }
                }
                Inner::EvalResult { target_node_id, eval_name, scores, grader_model } => {
                    NodeContent::EvalResult { target_node_id, eval_name, scores, grader_model }
                }
                Inner::ApprovalRequest { question, options, timeout_ms, context } => {
                    NodeContent::ApprovalRequest { question, options, timeout_ms, context }
                }
                Inner::ApprovalResponse { request_node_id, selected, comment, responded_by } => {
                    NodeContent::ApprovalResponse { request_node_id, selected, comment, responded_by }
                }
                Inner::WorkflowStep { step_name, step_index, total_steps, status, inputs, outputs, workflow_id } => {
                    NodeContent::WorkflowStep { step_name, step_index, total_steps, status, inputs, outputs, workflow_id }
                }
                Inner::VfsChange { path, operation } => NodeContent::VfsChange { path, operation },
            }),
            Err(_) => Ok(NodeContent::Unknown { kind: type_str, data: value }),
        }
    }
}

// ---------------------------------------------------------------------------
// NodeParams — builder for appending
// ---------------------------------------------------------------------------

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    Mcp,
    A2a,
    Skill,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaStreamType {
    ScreenRecording,
    Camera,
    Voice,
    NovaSonic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed { error: String },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationType {
    Comment,
    Highlight,
    Correction,
    Bookmark,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConversationIcon {
    Emoji { value: String },
    Svg { data: String },
    Image { storage_ref: StorageRef },
    Color { value: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageRef {
    pub backend: StorageBackend,
    pub uri: String,
    pub checksum: Option<String>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingState {
    pub started_at: i64,
    pub tokens_so_far: u64,
    pub last_chunk_at: i64,
    pub status: StreamStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamStatus {
    Active,
    Paused,
    Error { message: String },
    Cancelled,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reaction {
    pub node_id: NodeId,
    pub user_id: UserId,
    pub reaction_type: ReactionType,
    pub created_at: i64,
    pub comment: Option<String>,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetSplit {
    Train,
    Test,
    Eval,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEntryType {
    Fact,
    Preference,
    Goal,
    Skill,
    Custom(String),
}

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
mod tests {
    use super::*;
    use crate::types::ContentBlock;

    #[test]
    fn node_content_serde_round_trip_user_message() {
        let content = NodeContent::UserMessage {
            content: vec![ContentBlock::text("hello")],
            name: Some("alice".into()),
        };
        let json = serde_json::to_string(&content).unwrap();
        let parsed: NodeContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content_type_str(), "user_message");
    }

    #[test]
    fn node_content_serde_round_trip_assistant_message() {
        let content = NodeContent::AssistantMessage {
            content: vec![ContentBlock::text("hi")],
            stop_reason: Some(StopReason::EndTurn),
            variant_index: Some(0),
        };
        let json = serde_json::to_string(&content).unwrap();
        let parsed: NodeContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content_type_str(), "assistant_message");
    }

    #[test]
    fn node_content_unknown_forward_compat() {
        let json = r#"{"type": "future_type", "foo": "bar"}"#;
        let parsed: NodeContent = serde_json::from_str(json).unwrap();
        match &parsed {
            NodeContent::Unknown { kind, data } => {
                assert_eq!(kind, "future_type");
                assert_eq!(data["foo"], "bar");
            }
            other => panic!("Expected Unknown, got {}", other.content_type_str()),
        }
    }

    #[test]
    fn id_display_and_deref() {
        let id = NodeId::from_string("test-123");
        assert_eq!(id.as_str(), "test-123");
        assert_eq!(format!("{id}"), "test-123");
        assert_eq!(&*id, "test-123");
    }

    #[test]
    fn conversation_patch_maybe_cleared_round_trip() {
        let patch = ConversationPatch {
            instructions: Some(MaybeCleared::Set("do x".into())),
            ..Default::default()
        };
        let json = serde_json::to_string(&patch).unwrap();
        let back: ConversationPatch = serde_json::from_str(&json).unwrap();
        match back.instructions {
            Some(MaybeCleared::Set(s)) => assert_eq!(s, "do x"),
            other => panic!("Expected Set, got {other:?}"),
        }

        let clear_patch = ConversationPatch {
            instructions: Some(MaybeCleared::Clear),
            ..Default::default()
        };
        let json2 = serde_json::to_string(&clear_patch).unwrap();
        let back2: ConversationPatch = serde_json::from_str(&json2).unwrap();
        assert!(matches!(back2.instructions, Some(MaybeCleared::Clear)));
    }

    #[test]
    fn conversation_apply_patch() {
        let mut conv = Conversation::new("hello");
        conv.apply_patch(&ConversationPatch {
            title: Some("new title".into()),
            instructions: Some(MaybeCleared::Set("be helpful".into())),
            ..Default::default()
        });
        assert_eq!(conv.title.as_deref(), Some("new title"));
        assert_eq!(conv.instructions.as_deref(), Some("be helpful"));

        conv.apply_patch(&ConversationPatch {
            instructions: Some(MaybeCleared::Clear),
            ..Default::default()
        });
        assert!(conv.instructions.is_none());
    }
}
