//! Request/Response DTOs for OTel API

use serde::Serialize;

/// Partial success response for OTLP
#[derive(Debug, Serialize)]
pub struct PartialSuccess {
    pub rejected_spans: i64,
    pub error_message: Option<String>,
}

/// Export response
#[derive(Debug, Serialize, Default)]
pub struct ExportResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_success: Option<PartialSuccess>,
}

/// Health status for OTel subsystem
#[derive(Debug, Serialize)]
pub struct OtelHealthResponse {
    pub status: String,
    pub uptime_secs: u64,
    pub disk_usage_percent: Option<u8>,
    pub pending_spans: usize,
    pub active_sse_connections: usize,
}
