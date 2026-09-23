//! OpenAPI schemas for SideML values serialized by API DTOs.

use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolUse,
    ContentFilter,
    Error,
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        media_type: Option<String>,
        source: String,
        data: String,
        detail: Option<String>,
    },
    Audio {
        media_type: Option<String>,
        source: String,
        data: String,
    },
    Document {
        media_type: Option<String>,
        name: Option<String>,
        source: String,
        data: String,
    },
    Video {
        media_type: Option<String>,
        source: String,
        data: String,
    },
    File {
        media_type: Option<String>,
        name: Option<String>,
        source: String,
        data: String,
    },
    ToolUse {
        id: Option<String>,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: Option<String>,
        name: Option<String>,
        content: Value,
        is_error: bool,
    },
    ToolDefinitions {
        tools: Vec<Value>,
        tool_choice: Option<Value>,
    },
    Context {
        data: Value,
        context_type: Option<String>,
    },
    Refusal {
        message: String,
    },
    Json {
        data: Value,
    },
    Thinking {
        text: String,
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
    Unknown {
        raw: Value,
    },
}
