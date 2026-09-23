//! Stats API endpoint for project-level aggregations

use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Deserialize;

use super::OtelApiState;
use super::types::{
    CostsDto, CountsDto, FrameworkBreakdownDto, LatencyBucketDto, ModelBreakdownDto, PeriodDto,
    ProjectStatsDto, TokensDto, TrendBucketDto,
};
use crate::auth::ProjectRead;
use crate::types::{ApiError, parse_timestamp_param};
use sideseat_core::constants::CACHE_TTL_STATS;
use sideseat_ports::cache::{CacheKey, TypedCache};
use sideseat_ports::types::{ProjectStatsResult, StatsParams};

/// TTL for recent data (data from within the last 5 minutes) - 2 minutes
const CACHE_TTL_STATS_RECENT: u64 = 120;
const MAX_STATS_RANGE_DAYS: i64 = 90;

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub from_timestamp: String,
    pub to_timestamp: String,
    /// IANA timezone (e.g., "America/New_York"). Used for time bucketing.
    pub timezone: Option<String>,
}

pub(crate) const INVALID_TIMEZONE_MESSAGE: &str = "timezone must be a valid IANA timezone";
pub(crate) const INVALID_TIME_RANGE_MESSAGE: &str =
    "from_timestamp must be strictly before to_timestamp";
pub(crate) const RANGE_TOO_LARGE_MESSAGE: &str = "time range cannot exceed 90 days";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StatsRangeError {
    InvalidOrder,
    TooLarge,
}

pub(crate) fn normalize_timezone(timezone: Option<String>) -> Result<Tz, &'static str> {
    timezone.map_or(Ok(chrono_tz::UTC), |value| {
        value.parse().map_err(|_| INVALID_TIMEZONE_MESSAGE)
    })
}

pub(crate) fn validate_stats_time_range(
    from_timestamp: DateTime<Utc>,
    to_timestamp: DateTime<Utc>,
) -> Result<(), StatsRangeError> {
    if from_timestamp >= to_timestamp {
        return Err(StatsRangeError::InvalidOrder);
    }
    if to_timestamp - from_timestamp > chrono::Duration::days(MAX_STATS_RANGE_DAYS) {
        return Err(StatsRangeError::TooLarge);
    }
    Ok(())
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

    validate_stats_time_range(from_timestamp, to_timestamp).map_err(|error| match error {
        StatsRangeError::InvalidOrder => {
            ApiError::bad_request("INVALID_TIME_RANGE", INVALID_TIME_RANGE_MESSAGE)
        }
        StatsRangeError::TooLarge => {
            ApiError::bad_request("RANGE_TOO_LARGE", RANGE_TOO_LARGE_MESSAGE)
        }
    })?;

    let project_id = auth.project_id.clone();
    let timezone = normalize_timezone(query.timezone)
        .map_err(|message| ApiError::bad_request("INVALID_TIMEZONE", message))?;
    let cache = &state.cache;

    // Determine if this query is cacheable and calculate TTL
    // - Don't cache if to_timestamp is in the future (real-time)
    // - Use shorter TTL for recent data (within last 5 minutes)
    // - Use longer TTL for historical data
    let now = state.clock.now();
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
        from_timestamp.timestamp_micros(),
        to_timestamp.timestamp_micros(),
        timezone.name(),
    );

    // Try cache first (only if cacheable)
    if cache_ttl.is_some() {
        match cache.get::<ProjectStatsDto>(&cache_key).await {
            Ok(Some(cached_dto)) => {
                tracing::trace!(%project_id, "Stats cache hit");
                return Ok((stats_cache_headers(true), Json(cached_dto)));
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

    let repo = state.analytics.as_ref();
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

    Ok((stats_cache_headers(cache_ttl.is_some()), Json(dto)))
}

fn stats_cache_headers(cacheable: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if cacheable {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=60"),
        );
    }
    headers
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

#[cfg(test)]
mod tests {
    use axum::http::header;
    use chrono::{Duration, TimeZone, Utc};

    use super::{
        StatsRangeError, normalize_timezone, stats_cache_headers, validate_stats_time_range,
    };

    #[test]
    fn timezone_is_validated_and_canonicalized_before_caching() {
        assert_eq!(normalize_timezone(None), Ok(chrono_tz::UTC));
        assert_eq!(
            normalize_timezone(Some("Europe/London".into())),
            Ok(chrono_tz::Europe::London)
        );
        assert!(normalize_timezone(Some("not/a-timezone".into())).is_err());
    }

    #[test]
    fn stats_range_limit_is_exact_instead_of_rounded_to_whole_days() {
        let from = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

        assert_eq!(
            validate_stats_time_range(from, from + Duration::days(90)),
            Ok(())
        );
        assert_eq!(
            validate_stats_time_range(from, from + Duration::days(90) + Duration::microseconds(1)),
            Err(StatsRangeError::TooLarge)
        );
    }

    #[test]
    fn stats_range_requires_strictly_increasing_timestamps() {
        let timestamp = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

        assert_eq!(
            validate_stats_time_range(timestamp, timestamp),
            Err(StatsRangeError::InvalidOrder)
        );
        assert_eq!(
            validate_stats_time_range(timestamp, timestamp - Duration::microseconds(1)),
            Err(StatsRangeError::InvalidOrder)
        );
    }

    #[test]
    fn client_cache_policy_depends_on_query_cacheability_not_cache_hits() {
        assert_eq!(
            stats_cache_headers(true)
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("private, max-age=60")
        );
        assert!(
            stats_cache_headers(false)
                .get(header::CACHE_CONTROL)
                .is_none()
        );
    }
}
