use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Deserialize, JsonSchema)]
pub struct ListTracesInput {
    /// Max results (default: 20, max: 500)
    pub limit: Option<u32>,
    /// Page number (default: 1)
    pub page: Option<u32>,
    /// Filter by session ID
    pub session_id: Option<String>,
    /// Filter by environment
    pub environment: Option<String>,
    /// ISO 8601 start time
    pub from_timestamp: Option<String>,
    /// ISO 8601 end time
    pub to_timestamp: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetTraceInput {
    pub trace_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetMessagesInput {
    /// Trace ID (provide this OR session_id)
    pub trace_id: Option<String>,
    /// Span ID within a trace
    pub span_id: Option<String>,
    /// Session ID (provide this OR trace_id)
    pub session_id: Option<String>,
    /// Filter by role: user, assistant, system, tool
    pub role: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListSpansInput {
    pub limit: Option<u32>,
    pub page: Option<u32>,
    pub trace_id: Option<String>,
    pub session_id: Option<String>,
    /// Generation, Tool, Agent, Embedding, Chain, Retriever
    pub observation_type: Option<String>,
    pub framework: Option<String>,
    pub model: Option<String>,
    /// OK, ERROR
    pub status_code: Option<String>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetRawSpanInput {
    pub trace_id: String,
    pub span_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListSessionsInput {
    pub limit: Option<u32>,
    pub page: Option<u32>,
    pub user_id: Option<String>,
    pub environment: Option<String>,
    pub from_timestamp: Option<String>,
    pub to_timestamp: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetStatsInput {
    /// ISO 8601 period start (required)
    pub from_timestamp: String,
    /// ISO 8601 period end (required)
    pub to_timestamp: String,
    /// IANA timezone for trend bucketing (default: UTC)
    pub timezone: Option<String>,
}
