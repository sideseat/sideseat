use super::*;

/// Tagged union of all content variants a node can carry.
///
/// Serialized with a `"type"` discriminant (`snake_case`). Future schema versions
/// with unknown types deserialize as `Unknown { kind, data }` for forward compatibility.
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
            "user_message",
            "assistant_message",
            "system_message",
            "tool_result",
            "agent_spawn",
            "agent_result",
            "agent_event",
            "agent_handoff",
            "file_upload",
            "media_capture",
            "artifact_ref",
            "canvas_ref",
            "mcp_ui",
            "skill_invocation",
            "async_task",
            "computer_action",
            "mode_switch",
            "annotation",
            "source_ref",
            "eval_result",
            "approval_request",
            "approval_response",
            "workflow_step",
            "vfs_change",
        ];

        // Fast path: skip deserialization entirely for types not in this schema version.
        // This avoids cloning `value` for forward-compatibility unknown types.
        if !KNOWN_TYPES.contains(&type_str.as_str()) {
            return Ok(NodeContent::Unknown {
                kind: type_str,
                data: value,
            });
        }

        // Clone is unavoidable: `from_value` consumes `value`, but we need
        // it in the Err arm for graceful unknown fallback on schema mismatch.
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
                Inner::EvalResult {
                    target_node_id,
                    eval_name,
                    scores,
                    grader_model,
                } => NodeContent::EvalResult {
                    target_node_id,
                    eval_name,
                    scores,
                    grader_model,
                },
                Inner::ApprovalRequest {
                    question,
                    options,
                    timeout_ms,
                    context,
                } => NodeContent::ApprovalRequest {
                    question,
                    options,
                    timeout_ms,
                    context,
                },
                Inner::ApprovalResponse {
                    request_node_id,
                    selected,
                    comment,
                    responded_by,
                } => NodeContent::ApprovalResponse {
                    request_node_id,
                    selected,
                    comment,
                    responded_by,
                },
                Inner::WorkflowStep {
                    step_name,
                    step_index,
                    total_steps,
                    status,
                    inputs,
                    outputs,
                    workflow_id,
                } => NodeContent::WorkflowStep {
                    step_name,
                    step_index,
                    total_steps,
                    status,
                    inputs,
                    outputs,
                    workflow_id,
                },
                Inner::VfsChange { path, operation } => NodeContent::VfsChange { path, operation },
            }),
            Err(_) => Ok(NodeContent::Unknown {
                kind: type_str,
                data: value,
            }),
        }
    }
}
