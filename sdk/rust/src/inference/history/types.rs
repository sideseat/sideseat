use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{ContentBlock, StopReason, Usage};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const SCHEMA_VERSION: u32 = 1;
pub const INLINE_SIZE_LIMIT: usize = 1024 * 1024; // 1MB

// ---------------------------------------------------------------------------
// Newtype IDs — UUIDv7 (time-sortable)
// ---------------------------------------------------------------------------

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Deref for $name {
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
define_id!(ProjectId);

// ---------------------------------------------------------------------------
// Time helper
// ---------------------------------------------------------------------------

pub(crate) fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as i64
}

// ---------------------------------------------------------------------------
// Project
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub instructions: Option<String>,
    pub file_refs: Vec<StorageRef>,
    #[serde(default)]
    pub memory: HashMap<String, Value>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        let now = now_micros();
        Self {
            id: ProjectId::new(),
            name: name.into(),
            created_at: now,
            updated_at: now,
            instructions: None,
            file_refs: Vec::new(),
            memory: HashMap::new(),
            metadata: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Conversation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: ConversationId,
    pub project_id: Option<ProjectId>,
    pub title: Option<String>,
    pub icon: Option<ConversationIcon>,
    pub created_at: i64,
    pub updated_at: i64,
    pub created_by: Option<UserId>,
    pub default_model: Option<String>,
    pub default_provider: Option<String>,
    pub mode: ConversationMode,
    pub workspace: Workspace,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl Conversation {
    pub fn new(title: impl Into<String>) -> Self {
        let now = now_micros();
        Self {
            id: ConversationId::new(),
            project_id: None,
            title: Some(title.into()),
            icon: None,
            created_at: now,
            updated_at: now,
            created_by: None,
            default_model: None,
            default_provider: None,
            mode: ConversationMode::default(),
            workspace: Workspace::default(),
            metadata: HashMap::new(),
        }
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
    pub kanban_id: Option<String>,
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
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl Node {
    pub fn content_type(&self) -> &'static str {
        match &self.content {
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
            NodeContent::Unknown { .. } => "unknown",
        }
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

    // Forward compat
    Unknown {
        kind: String,
        data: Value,
    },
}

// Custom Deserialize for forward compat: unknown "type" → Unknown variant
impl<'de> Deserialize<'de> for NodeContent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let type_str = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        // Try standard deserialization first
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
        }

        match serde_json::from_value::<Inner>(value.clone()) {
            Ok(inner) => Ok(match inner {
                Inner::UserMessage { content, name } => NodeContent::UserMessage { content, name },
                Inner::AssistantMessage {
                    content,
                    stop_reason,
                    variant_index,
                } => NodeContent::AssistantMessage {
                    content,
                    stop_reason,
                    variant_index,
                },
                Inner::SystemMessage { content } => NodeContent::SystemMessage { content },
                Inner::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    duration_ms,
                } => NodeContent::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    duration_ms,
                },
                Inner::AgentSpawn {
                    agent_id,
                    agent_name,
                    sub_branch_id,
                    agent_type,
                    framework,
                    a2a_endpoint,
                    skill_id,
                    is_async,
                } => NodeContent::AgentSpawn {
                    agent_id,
                    agent_name,
                    sub_branch_id,
                    agent_type,
                    framework,
                    a2a_endpoint,
                    skill_id,
                    is_async,
                },
                Inner::AgentResult {
                    agent_id,
                    content,
                    usage_total,
                    was_async,
                } => NodeContent::AgentResult {
                    agent_id,
                    content,
                    usage_total,
                    was_async,
                },
                Inner::AgentEvent {
                    agent_id,
                    event_name,
                    event_data,
                } => NodeContent::AgentEvent {
                    agent_id,
                    event_name,
                    event_data,
                },
                Inner::AgentHandoff {
                    from_agent_id,
                    to_agent_id,
                    to_agent_name,
                    reason,
                    target_branch_id,
                } => NodeContent::AgentHandoff {
                    from_agent_id,
                    to_agent_id,
                    to_agent_name,
                    reason,
                    target_branch_id,
                },
                Inner::FileUpload {
                    file_id,
                    filename,
                    mime_type,
                    size_bytes,
                    storage_ref,
                } => NodeContent::FileUpload {
                    file_id,
                    filename,
                    mime_type,
                    size_bytes,
                    storage_ref,
                },
                Inner::MediaCapture {
                    stream_type,
                    started_at,
                    duration_ms,
                    storage_ref,
                    transcription,
                } => NodeContent::MediaCapture {
                    stream_type,
                    started_at,
                    duration_ms,
                    storage_ref,
                    transcription,
                },
                Inner::ArtifactRef {
                    artifact_set_id,
                    version,
                    summary,
                } => NodeContent::ArtifactRef {
                    artifact_set_id,
                    version,
                    summary,
                },
                Inner::CanvasRef {
                    canvas_id,
                    snapshot_version,
                    changed_items,
                } => NodeContent::CanvasRef {
                    canvas_id,
                    snapshot_version,
                    changed_items,
                },
                Inner::McpUi {
                    component_type,
                    component_data,
                    interactions,
                } => NodeContent::McpUi {
                    component_type,
                    component_data,
                    interactions,
                },
                Inner::SkillInvocation {
                    skill_id,
                    skill_name,
                    input,
                    output,
                    status,
                } => NodeContent::SkillInvocation {
                    skill_id,
                    skill_name,
                    input,
                    output,
                    status,
                },
                Inner::AsyncTask {
                    task_id,
                    task_type,
                    description,
                    status,
                    result,
                } => NodeContent::AsyncTask {
                    task_id,
                    task_type,
                    description,
                    status,
                    result,
                },
                Inner::ComputerAction {
                    action,
                    before_screenshot,
                    after_screenshot,
                    result,
                } => NodeContent::ComputerAction {
                    action,
                    before_screenshot,
                    after_screenshot,
                    result,
                },
                Inner::ModeSwitch { from_mode, to_mode } => {
                    NodeContent::ModeSwitch { from_mode, to_mode }
                }
                Inner::Annotation {
                    target_node_id,
                    text,
                    annotation_type,
                } => NodeContent::Annotation {
                    target_node_id,
                    text,
                    annotation_type,
                },
                Inner::SourceRef {
                    source_id,
                    source_name,
                    mime_type,
                    summary,
                } => NodeContent::SourceRef {
                    source_id,
                    source_name,
                    mime_type,
                    summary,
                },
            }),
            Err(_) => Ok(NodeContent::Unknown {
                kind: type_str.to_string(),
                data: value,
            }),
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
    pub parent_branch_id: Option<BranchId>,
    pub fork_node_id: Option<NodeId>,
    pub created_at: i64,
    pub created_by: Option<UserId>,
    pub name: Option<String>,
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
    fn node_content_serde_round_trip_tool_result() {
        let content = NodeContent::ToolResult {
            tool_use_id: "call_123".into(),
            content: vec![ContentBlock::text("result")],
            is_error: false,
            duration_ms: Some(100),
        };
        let json = serde_json::to_string(&content).unwrap();
        let parsed: NodeContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content_type_str(), "tool_result");
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
    fn node_content_type_correctness() {
        let variants: Vec<(&str, NodeContent)> = vec![
            (
                "user_message",
                NodeContent::UserMessage {
                    content: vec![],
                    name: None,
                },
            ),
            (
                "assistant_message",
                NodeContent::AssistantMessage {
                    content: vec![],
                    stop_reason: None,
                    variant_index: None,
                },
            ),
            (
                "system_message",
                NodeContent::SystemMessage { content: vec![] },
            ),
            (
                "agent_spawn",
                NodeContent::AgentSpawn {
                    agent_id: "a".into(),
                    agent_name: "a".into(),
                    sub_branch_id: BranchId::new(),
                    agent_type: AgentType::Mcp,
                    framework: None,
                    a2a_endpoint: None,
                    skill_id: None,
                    is_async: false,
                },
            ),
            (
                "mode_switch",
                NodeContent::ModeSwitch {
                    from_mode: None,
                    to_mode: ConversationMode::Agentic,
                },
            ),
            (
                "unknown",
                NodeContent::Unknown {
                    kind: "x".into(),
                    data: Value::Null,
                },
            ),
        ];

        for (expected, content) in variants {
            assert_eq!(content.content_type_str(), expected);
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
    fn conversation_mode_default() {
        assert_eq!(ConversationMode::default(), ConversationMode::Chat);
    }

    #[test]
    fn node_header_from_node() {
        let node = Node {
            id: NodeId::new(),
            conversation_id: ConversationId::new(),
            branch_id: BranchId::new(),
            parent_id: None,
            sequence: 1,
            created_at: now_micros(),
            created_by: None,
            model: Some("test-model".into()),
            provider: None,
            content: NodeContent::UserMessage {
                content: vec![ContentBlock::text("hi")],
                name: None,
            },
            usage: None,
            version: 0,
            is_final: true,
            streaming: None,
            deleted: false,
            metadata: HashMap::new(),
        };

        let header = NodeHeader::from(&node);
        assert_eq!(header.id, node.id);
        assert_eq!(header.content_type, "user_message");
        assert_eq!(header.model, Some("test-model".into()));
    }
}

impl NodeContent {
    #[cfg(test)]
    fn content_type_str(&self) -> &'static str {
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
            NodeContent::Unknown { .. } => "unknown",
        }
    }
}
