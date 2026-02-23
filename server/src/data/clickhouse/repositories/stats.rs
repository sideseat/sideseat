//! Stats repository for project-level aggregations (ClickHouse backend)

use chrono::{DateTime, Duration, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use clickhouse::{Client, Row};
use serde::Deserialize;

use super::query::{QueryParam, TOKEN_DEDUP_CONDITION, build_time_scoped_dedup};
use crate::core::constants::QUERY_MAX_TOP_STATS;
use crate::data::clickhouse::ClickhouseError;
use crate::data::types::{
    CostsResult, CountsResult, FrameworkBreakdown, LatencyBucket, ModelBreakdown,
    ProjectStatsResult, StatsParams, TokensResult, TrendBucket,
};

/// ClickHouse row for main aggregation
#[derive(Row, Deserialize)]
struct ChMainAggRow {
    traces: u64,
    sessions: u64,
    spans: u64,
    unique_users: u64,
    input_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    reasoning_tokens: i64,
    input_cost: f64,
    output_cost: f64,
    cache_read_cost: f64,
    cache_write_cost: f64,
    reasoning_cost: f64,
    total_cost: f64,
}

/// ClickHouse row for count query
#[derive(Row, Deserialize)]
struct ChCountRow {
    count: u64,
}

/// ClickHouse row for avg duration
#[derive(Row, Deserialize)]
struct ChAvgDurationRow {
    avg_duration_ms: f64,
}

/// ClickHouse row for framework breakdown
#[derive(Row, Deserialize)]
struct ChFrameworkRow {
    framework: Option<String>,
    count: u64,
    percentage: f64,
}

/// ClickHouse row for model breakdown
#[derive(Row, Deserialize)]
struct ChModelRow {
    model: Option<String>,
    tokens: i64,
    cost: f64,
    percentage: f64,
}

/// ClickHouse row for trend bucket
#[derive(Row, Deserialize)]
struct ChTrendRow {
    bucket: i64,
    tokens: i64,
}

/// ClickHouse row for latency bucket
#[derive(Row, Deserialize)]
struct ChLatencyRow {
    bucket: i64,
    avg_duration_ms: f64,
}

/// Get project stats for the given time range
pub async fn get_project_stats(
    client: &Client,
    params: &StatsParams,
) -> Result<ProjectStatsResult, ClickhouseError> {
    // Calculate previous period for comparison
    let period_duration = params.to_timestamp - params.from_timestamp;
    let prev_from = params.from_timestamp - period_duration;
    let prev_to = params.from_timestamp;

    // Determine granularity: if period > 48h, use daily buckets, else hourly
    let use_daily = period_duration > Duration::hours(48);

    // Query 1: Main aggregation (current period)
    let (counts, costs, tokens) = query_main_aggregation(client, params).await?;

    // Query 2: Previous period trace count
    let traces_previous = query_trace_count(client, &params.project_id, prev_from, prev_to).await?;

    // Query 3: Average trace duration
    let avg_trace_duration_ms = query_avg_trace_duration(client, params).await?;

    // Query 4: Framework breakdown
    let by_framework = query_framework_breakdown(client, params).await?;

    // Query 5: Model breakdown
    let by_model = query_model_breakdown(client, params).await?;

    // Query 6: Trend data
    let trend_data = query_trend_data(client, params, use_daily).await?;

    // Query 7: Latency trend data
    let latency_trend_data = query_latency_trend_data(client, params, use_daily).await?;

    // Query 8: Recent activity (last 5 minutes)
    let recent_activity_count = query_recent_activity(client, &params.project_id).await?;

    Ok(ProjectStatsResult {
        counts: CountsResult {
            traces: counts.traces as i64,
            traces_previous,
            sessions: counts.sessions as i64,
            spans: counts.spans as i64,
            unique_users: counts.unique_users as i64,
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
    traces: u64,
    sessions: u64,
    spans: u64,
    unique_users: u64,
}

async fn query_main_aggregation(
    client: &Client,
    params: &StatsParams,
) -> Result<(MainCounts, CostsResult, TokensResult), ClickhouseError> {
    let from_micros = params.from_timestamp.timestamp_micros();
    let to_micros = params.to_timestamp.timestamp_micros();

    let dedup = build_time_scoped_dedup(
        &params.project_id,
        Some(&params.from_timestamp),
        Some(&params.to_timestamp),
    );

    let sql = format!(
        r#"
        WITH {dedup_cte},
        gen_agg AS (
            SELECT
                coalesce(sum(gen_ai_usage_input_tokens), 0) AS input_tokens,
                coalesce(sum(gen_ai_usage_output_tokens), 0) AS output_tokens,
                coalesce(sum(gen_ai_usage_total_tokens), 0) AS total_tokens,
                coalesce(sum(gen_ai_usage_cache_read_tokens), 0) AS cache_read_tokens,
                coalesce(sum(gen_ai_usage_cache_write_tokens), 0) AS cache_write_tokens,
                coalesce(sum(gen_ai_usage_reasoning_tokens), 0) AS reasoning_tokens,
                coalesce(sum(toFloat64(gen_ai_cost_input)), 0) AS input_cost,
                coalesce(sum(toFloat64(gen_ai_cost_output)), 0) AS output_cost,
                coalesce(sum(toFloat64(gen_ai_cost_cache_read)), 0) AS cache_read_cost,
                coalesce(sum(toFloat64(gen_ai_cost_cache_write)), 0) AS cache_write_cost,
                coalesce(sum(toFloat64(gen_ai_cost_reasoning)), 0) AS reasoning_cost,
                coalesce(sum(toFloat64(gen_ai_cost_total)), 0) AS total_cost
            FROM otel_spans g FINAL
            WHERE g.project_id = ?
              AND g.timestamp_start >= fromUnixTimestamp64Micro(?)
              AND g.timestamp_start <= fromUnixTimestamp64Micro(?)
              AND {dedup_condition}
        )
        SELECT
            count(DISTINCT s.trace_id) AS traces,
            countIf(DISTINCT s.session_id, s.session_id IS NOT NULL) AS sessions,
            count() AS spans,
            countIf(DISTINCT s.user_id, s.user_id IS NOT NULL) AS unique_users,
            coalesce(max(ga.input_tokens), 0) AS input_tokens,
            coalesce(max(ga.output_tokens), 0) AS output_tokens,
            coalesce(max(ga.total_tokens), 0) AS total_tokens,
            coalesce(max(ga.cache_read_tokens), 0) AS cache_read_tokens,
            coalesce(max(ga.cache_write_tokens), 0) AS cache_write_tokens,
            coalesce(max(ga.reasoning_tokens), 0) AS reasoning_tokens,
            round(coalesce(max(ga.input_cost), 0), 4) AS input_cost,
            round(coalesce(max(ga.output_cost), 0), 4) AS output_cost,
            round(coalesce(max(ga.cache_read_cost), 0), 4) AS cache_read_cost,
            round(coalesce(max(ga.cache_write_cost), 0), 4) AS cache_write_cost,
            round(coalesce(max(ga.reasoning_cost), 0), 4) AS reasoning_cost,
            round(coalesce(max(ga.total_cost), 0), 4) AS total_cost
        FROM otel_spans s FINAL
        CROSS JOIN gen_agg ga
        WHERE s.project_id = ?
          AND s.timestamp_start >= fromUnixTimestamp64Micro(?)
          AND s.timestamp_start <= fromUnixTimestamp64Micro(?)
        "#,
        dedup_cte = dedup.0,
        dedup_condition = TOKEN_DEDUP_CONDITION,
    );

    // Bind order: dedup_lookup(project_id + time-scope), gen_agg(project_id, from, to), main(project_id, from, to)
    let mut q = client.query(&sql).bind(&params.project_id);
    for param in &dedup.1 {
        q = match param {
            QueryParam::String(s) => q.bind(s.as_str()),
            QueryParam::Int64(i) => q.bind(i),
        };
    }
    let row: ChMainAggRow = q
        .bind(&params.project_id)
        .bind(from_micros)
        .bind(to_micros)
        .bind(&params.project_id)
        .bind(from_micros)
        .bind(to_micros)
        .fetch_one()
        .await?;

    Ok((
        MainCounts {
            traces: row.traces,
            sessions: row.sessions,
            spans: row.spans,
            unique_users: row.unique_users,
        },
        CostsResult {
            input: row.input_cost,
            output: row.output_cost,
            cache_read: row.cache_read_cost,
            cache_write: row.cache_write_cost,
            reasoning: row.reasoning_cost,
            total: row.total_cost,
        },
        TokensResult {
            input: row.input_tokens,
            output: row.output_tokens,
            total: row.total_tokens,
            cache_read: row.cache_read_tokens,
            cache_write: row.cache_write_tokens,
            reasoning: row.reasoning_tokens,
        },
    ))
}

async fn query_trace_count(
    client: &Client,
    project_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<i64, ClickhouseError> {
    let from_micros = from.timestamp_micros();
    let to_micros = to.timestamp_micros();

    let sql = r#"
        SELECT count(DISTINCT trace_id) AS count
        FROM otel_spans FINAL
        WHERE project_id = ?
          AND timestamp_start >= fromUnixTimestamp64Micro(?)
          AND timestamp_start <= fromUnixTimestamp64Micro(?)
        "#;

    let row: ChCountRow = client
        .query(sql)
        .bind(project_id)
        .bind(from_micros)
        .bind(to_micros)
        .fetch_one()
        .await?;

    Ok(row.count as i64)
}

async fn query_avg_trace_duration(
    client: &Client,
    params: &StatsParams,
) -> Result<Option<f64>, ClickhouseError> {
    let from_micros = params.from_timestamp.timestamp_micros();
    let to_micros = params.to_timestamp.timestamp_micros();

    let sql = r#"
        WITH traces AS (
            SELECT
                trace_id,
                min(timestamp_start) AS min_ts,
                max(coalesce(timestamp_end, timestamp_start)) AS max_ts
            FROM otel_spans FINAL
            WHERE project_id = ?
              AND timestamp_start >= fromUnixTimestamp64Micro(?)
              AND timestamp_start <= fromUnixTimestamp64Micro(?)
            GROUP BY trace_id
        )
        SELECT avg(dateDiff('millisecond', min_ts, max_ts)) AS avg_duration_ms
        FROM traces
        "#;

    let row: ChAvgDurationRow = client
        .query(sql)
        .bind(&params.project_id)
        .bind(from_micros)
        .bind(to_micros)
        .fetch_one()
        .await?;

    Ok(Some(row.avg_duration_ms))
}

async fn query_framework_breakdown(
    client: &Client,
    params: &StatsParams,
) -> Result<Vec<FrameworkBreakdown>, ClickhouseError> {
    let from_micros = params.from_timestamp.timestamp_micros();
    let to_micros = params.to_timestamp.timestamp_micros();

    let sql = format!(
        r#"
        WITH genai_traces AS (
            SELECT DISTINCT trace_id
            FROM otel_spans FINAL
            WHERE project_id = ?
              AND timestamp_start >= fromUnixTimestamp64Micro(?)
              AND timestamp_start <= fromUnixTimestamp64Micro(?)
              AND observation_type != 'span'
        ),
        framework_counts AS (
            SELECT
                framework,
                count(DISTINCT trace_id) AS count
            FROM otel_spans FINAL
            WHERE project_id = ?
              AND timestamp_start >= fromUnixTimestamp64Micro(?)
              AND timestamp_start <= fromUnixTimestamp64Micro(?)
              AND trace_id IN (SELECT trace_id FROM genai_traces)
            GROUP BY framework
        ),
        total AS (
            SELECT coalesce(sum(count), 1) AS total FROM framework_counts
        )
        SELECT
            fc.framework AS framework,
            fc.count AS count,
            round(100.0 * fc.count / t.total, 1) AS percentage
        FROM framework_counts fc, total t
        ORDER BY fc.count DESC
        LIMIT {limit}
        "#,
        limit = QUERY_MAX_TOP_STATS
    );

    let rows: Vec<ChFrameworkRow> = client
        .query(&sql)
        .bind(&params.project_id)
        .bind(from_micros)
        .bind(to_micros)
        .bind(&params.project_id)
        .bind(from_micros)
        .bind(to_micros)
        .fetch_all()
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| FrameworkBreakdown {
            framework: r.framework,
            count: r.count as i64,
            percentage: r.percentage,
        })
        .collect())
}

async fn query_model_breakdown(
    client: &Client,
    params: &StatsParams,
) -> Result<Vec<ModelBreakdown>, ClickhouseError> {
    let from_micros = params.from_timestamp.timestamp_micros();
    let to_micros = params.to_timestamp.timestamp_micros();

    let dedup = build_time_scoped_dedup(
        &params.project_id,
        Some(&params.from_timestamp),
        Some(&params.to_timestamp),
    );

    let sql = format!(
        r#"
        WITH {dedup_cte},
        gen_roots AS (
            SELECT
                g.gen_ai_request_model,
                g.gen_ai_usage_total_tokens,
                g.gen_ai_cost_total
            FROM otel_spans g FINAL
            WHERE g.project_id = ?
              AND g.timestamp_start >= fromUnixTimestamp64Micro(?)
              AND g.timestamp_start <= fromUnixTimestamp64Micro(?)
              AND {dedup_condition}
        ),
        model_stats AS (
            SELECT
                gen_ai_request_model AS model,
                sum(gen_ai_usage_total_tokens) AS tokens,
                round(sum(toFloat64(gen_ai_cost_total)), 4) AS cost
            FROM gen_roots
            GROUP BY gen_ai_request_model
        ),
        total AS (
            SELECT coalesce(sum(tokens), 1) AS total FROM model_stats
        )
        SELECT
            ms.model AS model,
            ms.tokens AS tokens,
            ms.cost AS cost,
            round(100.0 * ms.tokens / t.total, 1) AS percentage
        FROM model_stats ms, total t
        ORDER BY ms.tokens DESC
        LIMIT {limit}
        "#,
        dedup_cte = dedup.0,
        dedup_condition = TOKEN_DEDUP_CONDITION,
        limit = QUERY_MAX_TOP_STATS
    );

    // Bind order: dedup_lookup(project_id + time-scope), gen_roots(project_id, from, to)
    let mut q = client.query(&sql).bind(&params.project_id);
    for param in &dedup.1 {
        q = match param {
            QueryParam::String(s) => q.bind(s.as_str()),
            QueryParam::Int64(i) => q.bind(i),
        };
    }
    let rows: Vec<ChModelRow> = q
        .bind(&params.project_id)
        .bind(from_micros)
        .bind(to_micros)
        .fetch_all()
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| ModelBreakdown {
            model: r.model,
            tokens: r.tokens,
            cost: r.cost,
            percentage: r.percentage,
        })
        .collect())
}

async fn query_trend_data(
    client: &Client,
    params: &StatsParams,
    use_daily: bool,
) -> Result<Vec<TrendBucket>, ClickhouseError> {
    let tz = parse_timezone(params.timezone.as_deref());
    let buckets =
        calculate_bucket_boundaries(params.from_timestamp, params.to_timestamp, tz, use_daily);

    if buckets.is_empty() {
        return Ok(Vec::new());
    }

    let from_micros = params.from_timestamp.timestamp_micros();
    let to_micros = params.to_timestamp.timestamp_micros();

    // Build bucket boundaries using fromUnixTimestamp64Micro with literal values
    // (safe: derived from internal DateTime calculations, not user input)
    let bucket_values: Vec<String> = buckets
        .iter()
        .map(|b| format!("fromUnixTimestamp64Micro({})", b.timestamp_micros()))
        .collect();
    let buckets_array = bucket_values.join(", ");

    let interval = if use_daily {
        "toIntervalDay(1)"
    } else {
        "toIntervalHour(1)"
    };

    let dedup = build_time_scoped_dedup(
        &params.project_id,
        Some(&params.from_timestamp),
        Some(&params.to_timestamp),
    );

    let sql = format!(
        r#"
        WITH all_buckets AS (
            SELECT arrayJoin([{buckets_array}]) AS bucket
        ),
        bucket_ranges AS (
            SELECT
                bucket,
                bucket AS bucket_start,
                bucket + {interval} AS bucket_end
            FROM all_buckets
        ),
        {dedup_cte},
        gen_roots AS (
            SELECT
                g.timestamp_start,
                coalesce(g.gen_ai_usage_total_tokens, 0) AS total_tokens
            FROM otel_spans g FINAL
            WHERE g.project_id = ?
              AND g.timestamp_start >= fromUnixTimestamp64Micro(?)
              AND g.timestamp_start <= fromUnixTimestamp64Micro(?)
              AND {dedup_condition}
        ),
        bucketed_data AS (
            SELECT
                br.bucket,
                sum(gr.total_tokens) AS tokens
            FROM gen_roots gr, bucket_ranges br
            WHERE gr.timestamp_start >= br.bucket_start
              AND gr.timestamp_start < br.bucket_end
            GROUP BY br.bucket
        )
        SELECT
            toInt64(toUnixTimestamp64Micro(ab.bucket)) AS bucket,
            coalesce(bd.tokens, 0) AS tokens
        FROM all_buckets ab
        LEFT JOIN bucketed_data bd ON bd.bucket = ab.bucket
        ORDER BY bucket ASC
        "#,
        buckets_array = buckets_array,
        interval = interval,
        dedup_cte = dedup.0,
        dedup_condition = TOKEN_DEDUP_CONDITION,
    );

    // Bind order: dedup_lookup(project_id + time-scope), gen_roots(project_id, from, to)
    let mut q = client.query(&sql).bind(&params.project_id);
    for param in &dedup.1 {
        q = match param {
            QueryParam::String(s) => q.bind(s.as_str()),
            QueryParam::Int64(i) => q.bind(i),
        };
    }
    let rows: Vec<ChTrendRow> = q
        .bind(&params.project_id)
        .bind(from_micros)
        .bind(to_micros)
        .fetch_all()
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| TrendBucket {
            bucket: DateTime::from_timestamp_micros(r.bucket).unwrap_or(DateTime::UNIX_EPOCH),
            tokens: r.tokens,
        })
        .collect())
}

async fn query_latency_trend_data(
    client: &Client,
    params: &StatsParams,
    use_daily: bool,
) -> Result<Vec<LatencyBucket>, ClickhouseError> {
    let tz = parse_timezone(params.timezone.as_deref());
    let buckets =
        calculate_bucket_boundaries(params.from_timestamp, params.to_timestamp, tz, use_daily);

    if buckets.is_empty() {
        return Ok(Vec::new());
    }

    let from_micros = params.from_timestamp.timestamp_micros();
    let to_micros = params.to_timestamp.timestamp_micros();

    // Build bucket boundaries using fromUnixTimestamp64Micro with literal values
    // (safe: derived from internal DateTime calculations, not user input)
    let bucket_values: Vec<String> = buckets
        .iter()
        .map(|b| format!("fromUnixTimestamp64Micro({})", b.timestamp_micros()))
        .collect();
    let buckets_array = bucket_values.join(", ");

    let interval = if use_daily {
        "toIntervalDay(1)"
    } else {
        "toIntervalHour(1)"
    };

    let sql = format!(
        r#"
        WITH all_buckets AS (
            SELECT arrayJoin([{buckets_array}]) AS bucket
        ),
        bucket_ranges AS (
            SELECT
                bucket,
                bucket AS bucket_start,
                bucket + {interval} AS bucket_end
            FROM all_buckets
        ),
        traces AS (
            SELECT
                trace_id,
                min(timestamp_start) AS min_ts,
                max(coalesce(timestamp_end, timestamp_start)) AS max_ts
            FROM otel_spans FINAL
            WHERE project_id = ?
              AND timestamp_start >= fromUnixTimestamp64Micro(?)
              AND timestamp_start <= fromUnixTimestamp64Micro(?)
            GROUP BY trace_id
        ),
        bucketed_data AS (
            SELECT
                br.bucket,
                avg(dateDiff('millisecond', t.min_ts, t.max_ts)) AS avg_duration_ms
            FROM traces t, bucket_ranges br
            WHERE t.min_ts >= br.bucket_start
              AND t.min_ts < br.bucket_end
            GROUP BY br.bucket
        )
        SELECT
            toInt64(toUnixTimestamp64Micro(ab.bucket)) AS bucket,
            coalesce(bd.avg_duration_ms, 0.0) AS avg_duration_ms
        FROM all_buckets ab
        LEFT JOIN bucketed_data bd ON bd.bucket = ab.bucket
        ORDER BY bucket ASC
        "#,
        buckets_array = buckets_array,
        interval = interval
    );

    let rows: Vec<ChLatencyRow> = client
        .query(&sql)
        .bind(&params.project_id)
        .bind(from_micros)
        .bind(to_micros)
        .fetch_all()
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| LatencyBucket {
            bucket: DateTime::from_timestamp_micros(r.bucket).unwrap_or(DateTime::UNIX_EPOCH),
            avg_duration_ms: r.avg_duration_ms,
        })
        .collect())
}

async fn query_recent_activity(client: &Client, project_id: &str) -> Result<i64, ClickhouseError> {
    let now = Utc::now();
    let five_min_ago = now - Duration::minutes(5);

    let from_micros = five_min_ago.timestamp_micros();
    let to_micros = now.timestamp_micros();

    let sql = r#"
        SELECT count(DISTINCT trace_id) AS count
        FROM otel_spans FINAL
        WHERE project_id = ?
          AND timestamp_start >= fromUnixTimestamp64Micro(?)
          AND timestamp_start <= fromUnixTimestamp64Micro(?)
        "#;

    let row: ChCountRow = client
        .query(sql)
        .bind(project_id)
        .bind(from_micros)
        .bind(to_micros)
        .fetch_one()
        .await?;

    Ok(row.count as i64)
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
    let mut current = match tz.from_local_datetime(&truncated_start) {
        chrono::LocalResult::Single(dt) => dt,
        chrono::LocalResult::Ambiguous(earliest, _) => earliest,
        chrono::LocalResult::None => {
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

    #[test]
    fn test_parse_timezone_valid() {
        assert_eq!(parse_timezone(Some("UTC")), chrono_tz::UTC);
        assert_eq!(
            parse_timezone(Some("America/New_York")),
            chrono_tz::America::New_York
        );
    }

    #[test]
    fn test_parse_timezone_invalid() {
        assert_eq!(parse_timezone(None), chrono_tz::UTC);
        assert_eq!(parse_timezone(Some("")), chrono_tz::UTC);
        assert_eq!(parse_timezone(Some("Invalid/Zone")), chrono_tz::UTC);
    }
}
