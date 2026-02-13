//! Shared stats types for all database backends
//!
//! This module contains stats query result types and parameters.

use chrono::{DateTime, Utc};

// ============================================================================
// Result types
// ============================================================================

/// Result structure for project stats
#[derive(Debug)]
pub struct ProjectStatsResult {
    pub counts: CountsResult,
    pub costs: CostsResult,
    pub tokens: TokensResult,
    pub by_framework: Vec<FrameworkBreakdown>,
    pub by_model: Vec<ModelBreakdown>,
    pub recent_activity_count: i64,
    pub avg_trace_duration_ms: Option<f64>,
    pub trend_data: Vec<TrendBucket>,
    pub latency_trend_data: Vec<LatencyBucket>,
}

#[derive(Debug, Default)]
pub struct CountsResult {
    pub traces: i64,
    pub traces_previous: i64,
    pub sessions: i64,
    pub spans: i64,
    pub unique_users: i64,
}

#[derive(Debug, Default)]
pub struct CostsResult {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub reasoning: f64,
    pub total: f64,
}

#[derive(Debug, Default)]
pub struct TokensResult {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
    pub total: i64,
}

#[derive(Debug)]
pub struct FrameworkBreakdown {
    pub framework: Option<String>,
    pub count: i64,
    pub percentage: f64,
}

#[derive(Debug)]
pub struct ModelBreakdown {
    pub model: Option<String>,
    pub tokens: i64,
    pub cost: f64,
    pub percentage: f64,
}

#[derive(Debug)]
pub struct TrendBucket {
    pub bucket: DateTime<Utc>,
    pub tokens: i64,
}

#[derive(Debug)]
pub struct LatencyBucket {
    pub bucket: DateTime<Utc>,
    pub avg_duration_ms: f64,
}

// ============================================================================
// Query parameters
// ============================================================================

/// Parameters for stats query
#[derive(Debug, Clone)]
pub struct StatsParams {
    pub project_id: String,
    pub from_timestamp: DateTime<Utc>,
    pub to_timestamp: DateTime<Utc>,
    /// IANA timezone (e.g., "America/New_York") for bucketing. Defaults to UTC.
    pub timezone: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stats_params() {
        let params = StatsParams {
            project_id: "test".to_string(),
            from_timestamp: chrono::Utc::now() - chrono::Duration::hours(24),
            to_timestamp: chrono::Utc::now(),
            timezone: None,
        };
        assert_eq!(params.project_id, "test");
    }
}
