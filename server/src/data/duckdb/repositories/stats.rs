//! Stats repository for project-level aggregations

use chrono::{DateTime, Duration, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use duckdb::Connection;

use crate::core::constants::QUERY_MAX_TOP_STATS;
use crate::data::duckdb::DuckdbError;
use crate::data::types::{
    CostsResult, CountsResult, FrameworkBreakdown, LatencyBucket, ModelBreakdown,
    ProjectStatsResult, StatsParams, TokensResult, TrendBucket,
};

/// Get project stats for the given time range
///
/// Note: These queries run sequentially because DuckDB's Rust driver is synchronous.
/// To parallelize, we'd need multiple connections and spawn_blocking/tokio::join!.
/// However, DuckDB is fast enough for these aggregations that the sequential approach
/// is acceptable for typical workloads. Consider parallelization if this becomes a bottleneck.
pub fn get_project_stats(
    conn: &Connection,
    params: &StatsParams,
) -> Result<ProjectStatsResult, DuckdbError> {
    // Calculate previous period for comparison
    let period_duration = params.to_timestamp - params.from_timestamp;
    let prev_from = params.from_timestamp - period_duration;
    let prev_to = params.from_timestamp;

    // Determine granularity: if period > 48h, use daily buckets, else hourly
    let use_daily = period_duration > Duration::hours(48);

    // Query 1: Main aggregation (current period)
    let (counts, costs, tokens) = query_main_aggregation(conn, params)?;

    // Query 2: Previous period trace count
    let traces_previous = query_trace_count(conn, &params.project_id, prev_from, prev_to)?;

    // Query 3: Average trace duration (uses trace boundary CTE similar to query 7)
    let avg_trace_duration_ms = query_avg_trace_duration(conn, params)?;

    // Query 4: Framework breakdown
    let by_framework = query_framework_breakdown(conn, params)?;

    // Query 5: Model breakdown
    let by_model = query_model_breakdown(conn, params)?;

    // Query 6: Trend data
    let trend_data = query_trend_data(conn, params, use_daily)?;

    // Query 7: Latency trend data (uses trace boundary CTE similar to query 3)
    let latency_trend_data = query_latency_trend_data(conn, params, use_daily)?;

    // Query 8: Recent activity (last 5 minutes, regardless of time range)
    let recent_activity_count = query_recent_activity(conn, &params.project_id)?;

    Ok(ProjectStatsResult {
        counts: CountsResult {
            traces: counts.traces,
            traces_previous,
            sessions: counts.sessions,
            spans: counts.spans,
            unique_users: counts.unique_users,
        },
        costs,
        tokens,
        by_framework,
        by_model,
        recent_activity_count,
        avg_trace_duration_ms,
        trend_data,
        latency_trend_data,
    })
}

/// Intermediate counts from main aggregation
struct MainCounts {
    traces: i64,
    sessions: i64,
    spans: i64,
    unique_users: i64,
}

fn query_main_aggregation(
    conn: &Connection,
    params: &StatsParams,
) -> Result<(MainCounts, CostsResult, TokensResult), DuckdbError> {
    // Two-path token filter to avoid double-counting.
    // See query.rs get_trace for detailed explanation.
    let sql = r#"
        WITH gen_agg AS (
            SELECT
                COALESCE(SUM(gen_ai_usage_input_tokens), 0) AS input_tokens,
                COALESCE(SUM(gen_ai_usage_output_tokens), 0) AS output_tokens,
                COALESCE(SUM(gen_ai_usage_total_tokens), 0) AS total_tokens,
                COALESCE(SUM(gen_ai_usage_cache_read_tokens), 0) AS cache_read_tokens,
                COALESCE(SUM(gen_ai_usage_cache_write_tokens), 0) AS cache_write_tokens,
                COALESCE(SUM(gen_ai_usage_reasoning_tokens), 0) AS reasoning_tokens,
                COALESCE(SUM(gen_ai_cost_input), 0) AS input_cost,
                COALESCE(SUM(gen_ai_cost_output), 0) AS output_cost,
                COALESCE(SUM(gen_ai_cost_cache_read), 0) AS cache_read_cost,
                COALESCE(SUM(gen_ai_cost_cache_write), 0) AS cache_write_cost,
                COALESCE(SUM(gen_ai_cost_reasoning), 0) AS reasoning_cost,
                COALESCE(SUM(gen_ai_cost_total), 0) AS total_cost
            FROM otel_spans g
            WHERE g.project_id = ?
              AND g.timestamp_start >= ?
              AND g.timestamp_start <= ?
              AND (
                  (g.observation_type = 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND (c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens) > 0
                   ))
                  OR
                  (g.observation_type != 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND (gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens) > 0
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.project_id = g.project_id
                         AND (p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens) > 0
                   ))
              )
        )
        SELECT
            COUNT(DISTINCT s.trace_id) AS traces,
            COUNT(DISTINCT s.session_id) FILTER (WHERE s.session_id IS NOT NULL) AS sessions,
            COUNT(*) AS spans,
            COUNT(DISTINCT s.user_id) FILTER (WHERE s.user_id IS NOT NULL) AS unique_users,
            COALESCE(MAX(ga.input_tokens), 0)::BIGINT AS input_tokens,
            COALESCE(MAX(ga.output_tokens), 0)::BIGINT AS output_tokens,
            COALESCE(MAX(ga.total_tokens), 0)::BIGINT AS total_tokens,
            COALESCE(MAX(ga.cache_read_tokens), 0)::BIGINT AS cache_read_tokens,
            COALESCE(MAX(ga.cache_write_tokens), 0)::BIGINT AS cache_write_tokens,
            COALESCE(MAX(ga.reasoning_tokens), 0)::BIGINT AS reasoning_tokens,
            ROUND(COALESCE(MAX(ga.input_cost), 0)::DOUBLE, 4) AS input_cost,
            ROUND(COALESCE(MAX(ga.output_cost), 0)::DOUBLE, 4) AS output_cost,
            ROUND(COALESCE(MAX(ga.cache_read_cost), 0)::DOUBLE, 4) AS cache_read_cost,
            ROUND(COALESCE(MAX(ga.cache_write_cost), 0)::DOUBLE, 4) AS cache_write_cost,
            ROUND(COALESCE(MAX(ga.reasoning_cost), 0)::DOUBLE, 4) AS reasoning_cost,
            ROUND(COALESCE(MAX(ga.total_cost), 0)::DOUBLE, 4) AS total_cost
        FROM otel_spans s
        CROSS JOIN gen_agg ga
        WHERE s.project_id = ?
          AND s.timestamp_start >= ?
          AND s.timestamp_start <= ?
    "#;

    let from_str = params.from_timestamp.to_rfc3339();
    let to_str = params.to_timestamp.to_rfc3339();

    let mut stmt = conn.prepare(sql)?;
    let row = stmt.query_row(
        [
            &params.project_id,
            &from_str,
            &to_str,
            &params.project_id,
            &from_str,
            &to_str,
        ],
        |row| {
            Ok((
                MainCounts {
                    traces: row.get(0)?,
                    sessions: row.get(1)?,
                    spans: row.get(2)?,
                    unique_users: row.get(3)?,
                },
                CostsResult {
                    input: row.get::<_, f64>(10)?,
                    output: row.get::<_, f64>(11)?,
                    cache_read: row.get::<_, f64>(12)?,
                    cache_write: row.get::<_, f64>(13)?,
                    reasoning: row.get::<_, f64>(14)?,
                    total: row.get::<_, f64>(15)?,
                },
                TokensResult {
                    input: row.get(4)?,
                    output: row.get(5)?,
                    total: row.get(6)?,
                    cache_read: row.get(7)?,
                    cache_write: row.get(8)?,
                    reasoning: row.get(9)?,
                },
            ))
        },
    )?;

    Ok(row)
}

fn query_trace_count(
    conn: &Connection,
    project_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<i64, DuckdbError> {
    let sql = r#"
        SELECT COUNT(DISTINCT trace_id) AS traces
        FROM otel_spans
        WHERE project_id = ?
          AND timestamp_start >= ?
          AND timestamp_start <= ?
    "#;

    let from_str = from.to_rfc3339();
    let to_str = to.to_rfc3339();

    let mut stmt = conn.prepare(sql)?;
    let count: i64 = stmt.query_row([project_id, &from_str, &to_str], |row| row.get(0))?;
    Ok(count)
}

fn query_avg_trace_duration(
    conn: &Connection,
    params: &StatsParams,
) -> Result<Option<f64>, DuckdbError> {
    let sql = r#"
        WITH traces AS (
            SELECT
                trace_id,
                MIN(timestamp_start) AS min_ts,
                MAX(COALESCE(timestamp_end, timestamp_start)) AS max_ts
            FROM otel_spans
            WHERE project_id = ?
              AND timestamp_start >= ?
              AND timestamp_start <= ?
            GROUP BY trace_id
        )
        SELECT AVG(DATE_DIFF('millisecond', min_ts, max_ts))::DOUBLE AS avg_duration_ms
        FROM traces
    "#;

    let from_str = params.from_timestamp.to_rfc3339();
    let to_str = params.to_timestamp.to_rfc3339();

    let mut stmt = conn.prepare(sql)?;
    let avg: Option<f64> =
        stmt.query_row([&params.project_id, &from_str, &to_str], |row| row.get(0))?;
    Ok(avg)
}

fn query_framework_breakdown(
    conn: &Connection,
    params: &StatsParams,
) -> Result<Vec<FrameworkBreakdown>, DuckdbError> {
    let sql = format!(
        r#"
        WITH genai_traces AS (
            SELECT DISTINCT trace_id
            FROM otel_spans
            WHERE project_id = ?
              AND timestamp_start >= ?
              AND timestamp_start <= ?
              AND observation_type != 'span'
        ),
        framework_counts AS (
            SELECT
                framework,
                COUNT(DISTINCT trace_id) AS count
            FROM otel_spans
            WHERE project_id = ?
              AND timestamp_start >= ?
              AND timestamp_start <= ?
              AND trace_id IN (SELECT trace_id FROM genai_traces)
            GROUP BY framework
        ),
        total AS (
            SELECT COALESCE(SUM(count), 1) AS total FROM framework_counts
        )
        SELECT
            fc.framework,
            fc.count,
            ROUND(100.0 * fc.count / t.total, 1) AS percentage
        FROM framework_counts fc, total t
        ORDER BY fc.count DESC
        LIMIT {}
    "#,
        QUERY_MAX_TOP_STATS
    );

    let from_str = params.from_timestamp.to_rfc3339();
    let to_str = params.to_timestamp.to_rfc3339();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        [
            &params.project_id,
            &from_str,
            &to_str,
            &params.project_id,
            &from_str,
            &to_str,
        ],
        |row| {
            Ok(FrameworkBreakdown {
                framework: row.get(0)?,
                count: row.get(1)?,
                percentage: row.get(2)?,
            })
        },
    )?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

fn query_model_breakdown(
    conn: &Connection,
    params: &StatsParams,
) -> Result<Vec<ModelBreakdown>, DuckdbError> {
    // Model breakdown is based on gen_ai_request_model from generation spans
    let sql = format!(
        r#"
        WITH gen_roots AS (
            SELECT
                g.gen_ai_request_model,
                g.gen_ai_usage_total_tokens,
                g.gen_ai_cost_total
            FROM otel_spans g
            WHERE g.project_id = ?
              AND g.timestamp_start >= ?
              AND g.timestamp_start <= ?
              AND (
                  (g.observation_type = 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND (c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens) > 0
                   ))
                  OR
                  (g.observation_type != 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND (gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens) > 0
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.project_id = g.project_id
                         AND (p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens) > 0
                   ))
              )
        ),
        model_stats AS (
            SELECT
                gen_ai_request_model AS model,
                COALESCE(SUM(gen_ai_usage_total_tokens), 0) AS tokens,
                ROUND(COALESCE(SUM(gen_ai_cost_total), 0)::DOUBLE, 4) AS cost
            FROM gen_roots
            GROUP BY gen_ai_request_model
        ),
        total AS (
            SELECT COALESCE(SUM(tokens), 1) AS total FROM model_stats
        )
        SELECT
            ms.model,
            ms.tokens,
            ms.cost,
            ROUND(100.0 * ms.tokens / t.total, 1) AS percentage
        FROM model_stats ms, total t
        ORDER BY ms.tokens DESC
        LIMIT {}
    "#,
        QUERY_MAX_TOP_STATS
    );

    let from_str = params.from_timestamp.to_rfc3339();
    let to_str = params.to_timestamp.to_rfc3339();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([&params.project_id, &from_str, &to_str], |row| {
        Ok(ModelBreakdown {
            model: row.get(0)?,
            tokens: row.get(1)?,
            cost: row.get(2)?,
            percentage: row.get(3)?,
        })
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

fn query_trend_data(
    conn: &Connection,
    params: &StatsParams,
    use_daily: bool,
) -> Result<Vec<TrendBucket>, DuckdbError> {
    let tz = parse_timezone(params.timezone.as_deref());
    let buckets =
        calculate_bucket_boundaries(params.from_timestamp, params.to_timestamp, tz, use_daily);

    if buckets.is_empty() {
        return Ok(Vec::new());
    }

    // Build VALUES clause for bucket boundaries
    let bucket_values: Vec<String> = buckets
        .iter()
        .map(|b| format!("('{}'::TIMESTAMP)", b.to_rfc3339()))
        .collect();
    let values_clause = bucket_values.join(", ");

    // Query: generate buckets from VALUES, LEFT JOIN with generation spans to sum tokens
    // Two-path token filter to avoid double-counting
    let sql = format!(
        r#"
        WITH all_buckets AS (
            SELECT col0 AS bucket FROM (VALUES {values_clause})
        ),
        bucket_ranges AS (
            SELECT
                bucket,
                bucket AS bucket_start,
                COALESCE(LEAD(bucket) OVER (ORDER BY bucket), bucket + INTERVAL '1 {}') AS bucket_end
            FROM all_buckets
        ),
        gen_roots AS (
            SELECT
                g.timestamp_start,
                COALESCE(g.gen_ai_usage_total_tokens, 0) AS total_tokens
            FROM otel_spans g
            WHERE g.project_id = ?
              AND g.timestamp_start >= ?
              AND g.timestamp_start <= ?
              AND (
                  (g.observation_type = 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans c
                       WHERE c.parent_span_id = g.span_id
                         AND c.project_id = g.project_id
                         AND c.observation_type = 'generation'
                         AND (c.gen_ai_usage_input_tokens + c.gen_ai_usage_output_tokens) > 0
                   ))
                  OR
                  (g.observation_type != 'generation'
                   AND (g.gen_ai_usage_input_tokens + g.gen_ai_usage_output_tokens) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans gen
                       WHERE gen.trace_id = g.trace_id
                         AND gen.project_id = g.project_id
                         AND gen.observation_type = 'generation'
                         AND (gen.gen_ai_usage_input_tokens + gen.gen_ai_usage_output_tokens) > 0
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM otel_spans p
                       WHERE p.span_id = g.parent_span_id
                         AND p.project_id = g.project_id
                         AND (p.gen_ai_usage_input_tokens + p.gen_ai_usage_output_tokens) > 0
                   ))
              )
        ),
        data_buckets AS (
            SELECT
                br.bucket,
                COALESCE(SUM(gr.total_tokens), 0)::BIGINT AS tokens
            FROM bucket_ranges br
            LEFT JOIN gen_roots gr ON
                gr.timestamp_start >= br.bucket_start
                AND gr.timestamp_start < br.bucket_end
            GROUP BY br.bucket
        )
        SELECT EPOCH_US(bucket) AS bucket, tokens
        FROM data_buckets
        ORDER BY bucket ASC
        "#,
        if use_daily { "day" } else { "hour" }
    );

    let from_str = params.from_timestamp.to_rfc3339();
    let to_str = params.to_timestamp.to_rfc3339();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([&params.project_id, &from_str, &to_str], |row| {
        let bucket_ts: i64 = row.get(0)?;
        let bucket = DateTime::from_timestamp_micros(bucket_ts)
            .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap());
        Ok(TrendBucket {
            bucket,
            tokens: row.get(1)?,
        })
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }

    Ok(result)
}

fn query_latency_trend_data(
    conn: &Connection,
    params: &StatsParams,
    use_daily: bool,
) -> Result<Vec<LatencyBucket>, DuckdbError> {
    let tz = parse_timezone(params.timezone.as_deref());
    let buckets =
        calculate_bucket_boundaries(params.from_timestamp, params.to_timestamp, tz, use_daily);

    if buckets.is_empty() {
        return Ok(Vec::new());
    }

    // Build VALUES clause for bucket boundaries
    let bucket_values: Vec<String> = buckets
        .iter()
        .map(|b| format!("('{}'::TIMESTAMP)", b.to_rfc3339()))
        .collect();
    let values_clause = bucket_values.join(", ");

    // Query: generate buckets from VALUES, LEFT JOIN with trace latency data
    let sql = format!(
        r#"
        WITH all_buckets AS (
            SELECT col0 AS bucket FROM (VALUES {values_clause})
        ),
        bucket_ranges AS (
            SELECT
                bucket,
                bucket AS bucket_start,
                COALESCE(LEAD(bucket) OVER (ORDER BY bucket), bucket + INTERVAL '1 {}') AS bucket_end
            FROM all_buckets
        ),
        traces AS (
            SELECT
                trace_id,
                MIN(timestamp_start) AS min_ts,
                MAX(COALESCE(timestamp_end, timestamp_start)) AS max_ts
            FROM otel_spans
            WHERE project_id = ?
              AND timestamp_start >= ?
              AND timestamp_start <= ?
            GROUP BY trace_id
        ),
        data_buckets AS (
            SELECT
                br.bucket,
                AVG(DATE_DIFF('millisecond', t.min_ts, t.max_ts))::DOUBLE AS avg_duration_ms
            FROM bucket_ranges br
            LEFT JOIN traces t ON
                t.min_ts >= br.bucket_start
                AND t.min_ts < br.bucket_end
            GROUP BY br.bucket
        )
        SELECT EPOCH_US(bucket) AS bucket, COALESCE(avg_duration_ms, 0.0) AS avg_duration_ms
        FROM data_buckets
        ORDER BY bucket ASC
        "#,
        if use_daily { "day" } else { "hour" }
    );

    let from_str = params.from_timestamp.to_rfc3339();
    let to_str = params.to_timestamp.to_rfc3339();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([&params.project_id, &from_str, &to_str], |row| {
        let bucket_ts: i64 = row.get(0)?;
        let bucket = DateTime::from_timestamp_micros(bucket_ts)
            .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap());
        Ok(LatencyBucket {
            bucket,
            avg_duration_ms: row.get(1)?,
        })
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }

    Ok(result)
}

/// Parse timezone string using chrono-tz.
/// Returns UTC if the timezone is invalid or None.
fn parse_timezone(tz: Option<&str>) -> Tz {
    tz.and_then(|s| s.parse::<Tz>().ok())
        .unwrap_or(chrono_tz::UTC)
}

/// Calculate bucket boundaries in a given timezone.
/// Returns a list of UTC timestamps representing the start of each bucket.
fn calculate_bucket_boundaries(
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    tz: Tz,
    use_daily: bool,
) -> Vec<DateTime<Utc>> {
    let mut buckets = Vec::new();

    // Convert to local timezone
    let local_from = from.with_timezone(&tz);
    let local_to = to.with_timezone(&tz);

    // Truncate to start of hour/day in local time
    let truncated_start = if use_daily {
        local_from
            .date_naive()
            .and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())
    } else {
        local_from
            .date_naive()
            .and_hms_opt(local_from.hour(), 0, 0)
            .unwrap()
    };

    // Convert back to timezone-aware datetime
    // Use earliest() to handle ambiguous times during DST fall-back
    // If time doesn't exist (DST spring-forward gap), try adding the step interval
    let mut current = match tz.from_local_datetime(&truncated_start) {
        chrono::LocalResult::Single(dt) => dt,
        chrono::LocalResult::Ambiguous(earliest, _) => earliest,
        chrono::LocalResult::None => {
            // Time falls in DST gap - try to find next valid time
            let step = if use_daily {
                Duration::days(1)
            } else {
                Duration::hours(1)
            };
            match tz.from_local_datetime(&(truncated_start + step)) {
                chrono::LocalResult::Single(dt) => dt,
                chrono::LocalResult::Ambiguous(earliest, _) => earliest,
                chrono::LocalResult::None => return buckets,
            }
        }
    };

    let step = if use_daily {
        Duration::days(1)
    } else {
        Duration::hours(1)
    };

    while current <= local_to {
        buckets.push(current.with_timezone(&Utc));
        current += step;
    }

    buckets
}

fn query_recent_activity(conn: &Connection, project_id: &str) -> Result<i64, DuckdbError> {
    // Count distinct traces in the last 5 minutes (regardless of time range selection)
    let now = Utc::now();
    let five_min_ago = (now - Duration::minutes(5)).to_rfc3339();
    let now_str = now.to_rfc3339();

    let sql = r#"
        SELECT COUNT(DISTINCT trace_id)
        FROM otel_spans
        WHERE project_id = ?
          AND timestamp_start >= ?
          AND timestamp_start <= ?
    "#;

    let mut stmt = conn.prepare(sql)?;
    let count: i64 = stmt.query_row([project_id, &five_min_ago, &now_str], |row| row.get(0))?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn test_parse_timezone_valid() {
        // Standard IANA timezones
        assert_eq!(parse_timezone(Some("UTC")), chrono_tz::UTC);
        assert_eq!(
            parse_timezone(Some("America/New_York")),
            chrono_tz::America::New_York
        );
        assert_eq!(
            parse_timezone(Some("Europe/London")),
            chrono_tz::Europe::London
        );
        assert_eq!(
            parse_timezone(Some("Europe/Berlin")),
            chrono_tz::Europe::Berlin
        );
        assert_eq!(parse_timezone(Some("Asia/Tokyo")), chrono_tz::Asia::Tokyo);
    }

    #[test]
    fn test_parse_timezone_invalid() {
        // Invalid timezones default to UTC
        assert_eq!(parse_timezone(None), chrono_tz::UTC);
        assert_eq!(parse_timezone(Some("")), chrono_tz::UTC);
        assert_eq!(parse_timezone(Some("Invalid/Zone")), chrono_tz::UTC);
        assert_eq!(parse_timezone(Some("'; DROP TABLE")), chrono_tz::UTC);
    }

    #[test]
    fn test_calculate_bucket_boundaries_hourly() {
        let from = DateTime::parse_from_rfc3339("2024-01-17T10:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2024-01-17T13:45:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let buckets = calculate_bucket_boundaries(from, to, chrono_tz::UTC, false);

        assert_eq!(buckets.len(), 4); // 10:00, 11:00, 12:00, 13:00
        assert_eq!(buckets[0].hour(), 10);
        assert_eq!(buckets[1].hour(), 11);
        assert_eq!(buckets[2].hour(), 12);
        assert_eq!(buckets[3].hour(), 13);
    }

    #[test]
    fn test_calculate_bucket_boundaries_daily() {
        let from = DateTime::parse_from_rfc3339("2024-01-15T10:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2024-01-17T13:45:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let buckets = calculate_bucket_boundaries(from, to, chrono_tz::UTC, true);

        assert_eq!(buckets.len(), 3); // Jan 15, 16, 17
        assert_eq!(buckets[0].day(), 15);
        assert_eq!(buckets[1].day(), 16);
        assert_eq!(buckets[2].day(), 17);
    }

    #[test]
    fn test_calculate_bucket_boundaries_with_timezone() {
        // Test with Europe/Berlin (UTC+1 in January)
        // 2024-01-17 02:30 UTC = 2024-01-17 03:30 Berlin
        let from = DateTime::parse_from_rfc3339("2024-01-17T02:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2024-01-17T05:30:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let buckets = calculate_bucket_boundaries(from, to, chrono_tz::Europe::Berlin, false);

        // In Berlin time: 03:30 to 06:30 = buckets at 03:00, 04:00, 05:00, 06:00
        // Converted back to UTC: 02:00, 03:00, 04:00, 05:00
        assert_eq!(buckets.len(), 4);
        assert_eq!(buckets[0].hour(), 2); // 03:00 Berlin = 02:00 UTC
        assert_eq!(buckets[1].hour(), 3); // 04:00 Berlin = 03:00 UTC
        assert_eq!(buckets[2].hour(), 4); // 05:00 Berlin = 04:00 UTC
        assert_eq!(buckets[3].hour(), 5); // 06:00 Berlin = 05:00 UTC
    }

    #[test]
    fn test_calculate_bucket_boundaries_daily_with_timezone() {
        // Test day boundaries with timezone offset
        // Europe/Berlin is UTC+1 in January
        // When it's 2024-01-16 23:30 UTC, it's 2024-01-17 00:30 in Berlin
        // When it's 2024-01-16 23:00 UTC, it's 2024-01-17 00:00 in Berlin (start of day)
        let from = DateTime::parse_from_rfc3339("2024-01-16T23:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2024-01-17T23:30:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let buckets = calculate_bucket_boundaries(from, to, chrono_tz::Europe::Berlin, true);

        // Berlin times: Jan 17 00:30 to Jan 18 00:30
        // Day buckets: Jan 17 00:00 Berlin (= Jan 16 23:00 UTC), Jan 18 00:00 Berlin (= Jan 17 23:00 UTC)
        assert_eq!(buckets.len(), 2);
        // Jan 17 00:00 Berlin = Jan 16 23:00 UTC
        assert_eq!(buckets[0].day(), 16);
        assert_eq!(buckets[0].hour(), 23);
        // Jan 18 00:00 Berlin = Jan 17 23:00 UTC
        assert_eq!(buckets[1].day(), 17);
        assert_eq!(buckets[1].hour(), 23);
    }
}
