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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_state_serialization() {
        assert_eq!(serde_json::to_string(&HealthState::Healthy).unwrap(), "\"healthy\"");
        assert_eq!(serde_json::to_string(&HealthState::Degraded).unwrap(), "\"degraded\"");
        assert_eq!(serde_json::to_string(&HealthState::Unhealthy).unwrap(), "\"unhealthy\"");
    }

    #[test]
    fn test_component_health_serialization() {
        let health =
            ComponentHealth { status: HealthState::Healthy, message: None, last_activity: None };
        let json = serde_json::to_string(&health).unwrap();
        assert!(json.contains("\"status\":\"healthy\""));
        assert!(!json.contains("message"));
        assert!(!json.contains("last_activity"));
    }

    #[test]
    fn test_component_health_with_message() {
        let health = ComponentHealth {
            status: HealthState::Degraded,
            message: Some("High latency detected".to_string()),
            last_activity: Some(1234567890),
        };
        let json = serde_json::to_string(&health).unwrap();
        assert!(json.contains("\"status\":\"degraded\""));
        assert!(json.contains("\"message\":\"High latency detected\""));
        assert!(json.contains("\"last_activity\":1234567890"));
    }

    #[test]
    fn test_otel_stats_serialization() {
        let stats = OtelStats {
            total_traces: 100,
            total_spans: 500,
            storage_bytes: 1024 * 1024,
            storage_files: 10,
            disk_usage_percent: 45,
            buffer_size: 50,
            buffer_capacity: 1000,
            sse_connections: 5,
            uptime_seconds: 3600,
        };
        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("\"total_traces\":100"));
        assert!(json.contains("\"total_spans\":500"));
        assert!(json.contains("\"disk_usage_percent\":45"));
    }

    #[test]
    fn test_otel_component_status_serialization() {
        let status = OtelComponentStatus {
            http_collector: ComponentHealth {
                status: HealthState::Healthy,
                message: None,
                last_activity: None,
            },
            grpc_collector: ComponentHealth {
                status: HealthState::Unhealthy,
                message: Some("gRPC disabled".to_string()),
                last_activity: None,
            },
            sqlite: ComponentHealth {
                status: HealthState::Healthy,
                message: None,
                last_activity: None,
            },
            parquet_writer: ComponentHealth {
                status: HealthState::Healthy,
                message: None,
                last_activity: None,
            },
            sse_manager: ComponentHealth {
                status: HealthState::Healthy,
                message: None,
                last_activity: None,
            },
            retention_manager: ComponentHealth {
                status: HealthState::Healthy,
                message: None,
                last_activity: None,
            },
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("http_collector"));
        assert!(json.contains("grpc_collector"));
        assert!(json.contains("sqlite"));
    }

    #[test]
    fn test_otel_health_status_full() {
        let health = OtelHealthStatus {
            enabled: true,
            status: HealthState::Healthy,
            components: OtelComponentStatus {
                http_collector: ComponentHealth {
                    status: HealthState::Healthy,
                    message: None,
                    last_activity: None,
                },
                grpc_collector: ComponentHealth {
                    status: HealthState::Healthy,
                    message: None,
                    last_activity: None,
                },
                sqlite: ComponentHealth {
                    status: HealthState::Healthy,
                    message: None,
                    last_activity: None,
                },
                parquet_writer: ComponentHealth {
                    status: HealthState::Healthy,
                    message: None,
                    last_activity: None,
                },
                sse_manager: ComponentHealth {
                    status: HealthState::Healthy,
                    message: None,
                    last_activity: None,
                },
                retention_manager: ComponentHealth {
                    status: HealthState::Healthy,
                    message: None,
                    last_activity: None,
                },
            },
            stats: OtelStats {
                total_traces: 0,
                total_spans: 0,
                storage_bytes: 0,
                storage_files: 0,
                disk_usage_percent: 0,
                buffer_size: 0,
                buffer_capacity: 1000,
                sse_connections: 0,
                uptime_seconds: 0,
            },
        };
        let json = serde_json::to_string(&health).unwrap();
        assert!(json.contains("\"enabled\":true"));
        assert!(json.contains("\"status\":\"healthy\""));
        assert!(json.contains("\"components\""));
        assert!(json.contains("\"stats\""));
    }

    #[test]
    fn test_health_state_clone() {
        let state = HealthState::Degraded;
        let cloned = state.clone();
        assert!(matches!(cloned, HealthState::Degraded));
    }

    #[test]
    fn test_health_state_debug() {
        let state = HealthState::Unhealthy;
        let debug = format!("{:?}", state);
        assert_eq!(debug, "Unhealthy");
    }
}
