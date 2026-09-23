//! Log read models and query parameters.

use chrono::{DateTime, Utc};

use super::ProjectId;

#[derive(Debug, Clone)]
pub struct LogRow {
    pub log_digest: String,
    pub ordinal: u32,
    pub timestamp: DateTime<Utc>,
    pub time: Option<DateTime<Utc>>,
    pub observed_time: Option<DateTime<Utc>>,
    pub severity_number: i32,
    pub severity_text: Option<String>,
    pub body: Option<String>,
    pub body_text: Option<String>,
    pub attributes: Option<String>,
    pub dropped_attributes_count: u32,
    pub flags: u32,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub event_name: Option<String>,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub environment: Option<String>,
    pub service_name: Option<String>,
    pub service_version: Option<String>,
    pub service_namespace: Option<String>,
    pub service_instance_id: Option<String>,
    pub resource_attributes: Option<String>,
    pub scope_name: Option<String>,
    pub scope_version: Option<String>,
    pub scope_attributes: Option<String>,
    pub scope_schema_url: Option<String>,
    pub resource_schema_url: Option<String>,
    pub raw_log: Option<String>,
    pub ingested_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct ListLogsParams {
    pub project_id: ProjectId,
    pub page: u32,
    pub limit: u32,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub severity_text: Option<String>,
    pub severity_number_min: Option<i32>,
    pub service_name: Option<String>,
    pub environment: Option<String>,
    pub from_timestamp: Option<DateTime<Utc>>,
    pub to_timestamp: Option<DateTime<Utc>>,
}
