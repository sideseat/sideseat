//! Stats API endpoint for project-level aggregations

use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::OtelApiState;
use super::types::{
    CostsDto, CountsDto, FrameworkBreakdownDto, LatencyBucketDto, ModelBreakdownDto, PeriodDto,
    ProjectStatsDto, TokensDto, TrendBucketDto,
};
use crate::api::auth::ProjectRead;
use crate::api::types::{ApiError, parse_timestamp_param};
use crate::core::constants::CACHE_TTL_STATS;
use crate::data::cache::CacheKey;
use crate::data::types::{ProjectStatsResult, StatsParams};

/// TTL for recent data (data from within the last 5 minutes) - 2 minutes
const CACHE_TTL_STATS_RECENT: u64 = 120;

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub from_timestamp: String,
    pub to_timestamp: String,
    /// IANA timezone (e.g., "America/New_York"). Used for time bucketing.
    pub timezone: Option<String>,
}

/// Get project stats for the given time range
#[utoipa::path(
    get,
    path = "/api/v1/project/{project_id}/otel/stats",
    tag = "stats",
    params(
        ("project_id" = String, Path, description = "Project ID"),
        ("from_timestamp" = String, Query, description = "Start of time range (ISO 8601, required)"),
        ("to_timestamp" = String, Query, description = "End of time range (ISO 8601, required)"),
        ("timezone" = Option<String>, Query, description = "IANA timezone for bucketing (e.g., America/New_York)")
    ),
    responses(
        (status = 200, description = "Project stats for the given time range")
    )
)]
pub async fn get_project_stats(
    State(state): State<OtelApiState>,
    auth: ProjectRead,
    Query(query): Query<StatsQuery>,
) -> Result<(HeaderMap, Json<ProjectStatsDto>), ApiError> {
    // Parse timestamps (required)
    let from_timestamp = parse_timestamp_param(&Some(query.from_timestamp))?
        .ok_or_else(|| ApiError::bad_request("MISSING_PARAM", "from_timestamp is required"))?;

    let to_timestamp = parse_timestamp_param(&Some(query.to_timestamp))?
        .ok_or_else(|| ApiError::bad_request("MISSING_PARAM", "to_timestamp is required"))?;

    // Validate time range
    if from_timestamp >= to_timestamp {
        return Err(ApiError::bad_request(
            "INVALID_TIME_RANGE",
            "from_timestamp must be strictly before to_timestamp",
        ));
    }

    // Limit max range to prevent excessive bucket generation
    let range_days = (to_timestamp - from_timestamp).num_days();
    if range_days > 90 {
        return Err(ApiError::bad_request(
            "RANGE_TOO_LARGE",
            "Time range cannot exceed 90 days",
        ));
    }

    let project_id = auth.project_id.clone();
    let timezone = query.timezone.clone();
    let cache = &state.cache;

    // Determine if this query is cacheable and calculate TTL
    // - Don't cache if to_timestamp is in the future (real-time)
    // - Use shorter TTL for recent data (within last 5 minutes)
    // - Use longer TTL for historical data
    let now = Utc::now();
    let cache_ttl = if to_timestamp > now {
        // Real-time query (extends into future) - don't cache
        None
    } else {
        let seconds_ago = (now - to_timestamp).num_seconds();
        if seconds_ago < 300 {
            // Recent data (within 5 minutes) - short TTL
            Some(Duration::from_secs(CACHE_TTL_STATS_RECENT))
        } else {
            // Historical data - longer TTL
            Some(Duration::from_secs(CACHE_TTL_STATS))
        }
    };

    // Generate cache key
    let cache_key = CacheKey::stats(
        &project_id,
        from_timestamp.timestamp(),
        to_timestamp.timestamp(),
        timezone.as_deref().unwrap_or("UTC"),
    );

    // Try cache first (only if cacheable)
    if cache_ttl.is_some() {
        match cache.get::<ProjectStatsDto>(&cache_key).await {
            Ok(Some(cached_dto)) => {
                tracing::trace!(%project_id, "Stats cache hit");
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("private, max-age=60"),
                );
                return Ok((headers, Json(cached_dto)));
            }
            Err(e) => tracing::warn!(%project_id, error = %e, "Stats cache get error"),
            Ok(None) => {}
        }
    }

    // Cache miss or non-cacheable - run query
    let params = StatsParams {
        project_id: project_id.clone(),
        from_timestamp,
        to_timestamp,
        timezone,
    };

    let repo = state.analytics.repository();
    let result = repo
        .get_project_stats(&params)
        .await
        .map_err(ApiError::from_data)?;

    let dto = stats_result_to_dto(result, from_timestamp, to_timestamp);

    // Store in cache if cacheable
    if let Some(ttl) = cache_ttl
        && let Err(e) = cache.set(&cache_key, &dto, Some(ttl)).await
    {
        tracing::warn!(%project_id, error = %e, "Stats cache set error");
    }

    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));

    Ok((headers, Json(dto)))
}

pub(crate) fn stats_result_to_dto(
    result: ProjectStatsResult,
    from_timestamp: DateTime<Utc>,
    to_timestamp: DateTime<Utc>,
) -> ProjectStatsDto {
    ProjectStatsDto {
        period: PeriodDto {
            from: from_timestamp,
            to: to_timestamp,
        },
        counts: CountsDto {
            traces: result.counts.traces,
            traces_previous: result.counts.traces_previous,
            sessions: result.counts.sessions,
            spans: result.counts.spans,
            unique_users: result.counts.unique_users,
        },
        costs: CostsDto {
            input: result.costs.input,
            output: result.costs.output,
            cache_read: result.costs.cache_read,
            cache_write: result.costs.cache_write,
            reasoning: result.costs.reasoning,
            total: result.costs.total,
        },
        tokens: TokensDto {
            input: result.tokens.input,
            output: result.tokens.output,
            cache_read: result.tokens.cache_read,
            cache_write: result.tokens.cache_write,
            reasoning: result.tokens.reasoning,
            total: result.tokens.total,
        },
        by_framework: result
            .by_framework
            .into_iter()
            .map(|f| FrameworkBreakdownDto {
                framework: f.framework,
                count: f.count,
                percentage: f.percentage,
            })
            .collect(),
        by_model: result
            .by_model
            .into_iter()
            .map(|m| ModelBreakdownDto {
                model: m.model,
                tokens: m.tokens,
                cost: m.cost,
                percentage: m.percentage,
            })
            .collect(),
        recent_activity_count: result.recent_activity_count,
        avg_trace_duration_ms: result.avg_trace_duration_ms,
        trend_data: result
            .trend_data
            .into_iter()
            .map(|t| TrendBucketDto {
                bucket: t.bucket,
                tokens: t.tokens,
            })
            .collect(),
        latency_trend_data: result
            .latency_trend_data
            .into_iter()
            .map(|t| LatencyBucketDto {
                bucket: t.bucket,
                avg_duration_ms: t.avg_duration_ms,
            })
            .collect(),
    }
}
