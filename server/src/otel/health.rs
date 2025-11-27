//! OTel health status types

use serde::Serialize;

/// OTel subsystem health status
#[derive(Debug, Clone, Serialize)]
pub struct OtelHealthStatus {
    pub enabled: bool,
    pub status: HealthState,
    pub components: OtelComponentStatus,
    pub stats: OtelStats,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone, Serialize)]
pub struct OtelComponentStatus {
    pub http_collector: ComponentHealth,
    pub grpc_collector: ComponentHealth,
    pub sqlite: ComponentHealth,
    pub parquet_writer: ComponentHealth,
    pub sse_manager: ComponentHealth,
    pub retention_manager: ComponentHealth,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComponentHealth {
    pub status: HealthState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OtelStats {
    pub total_traces: u64,
    pub total_spans: u64,
    pub storage_bytes: u64,
    pub storage_files: u64,
    pub disk_usage_percent: u8,
    pub buffer_size: usize,
    pub buffer_capacity: usize,
    pub sse_connections: u64,
    pub uptime_seconds: u64,
}
