//! ClickHouse execution and decoding for the shared project-statistics plan.

use chrono::{DateTime, Duration, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use clickhouse::{Client, Row, RowOwned, RowRead};
use serde::Deserialize;
use sideseat_ports::types::{
    CostsResult, CountsResult, FrameworkBreakdown, LatencyBucket, ModelBreakdown,
    ProjectStatsResult, StatsParams, TokensResult, TrendBucket,
};
use sideseat_query_sql::Backend;
use sideseat_query_sql::analytics::ParameterizedQuery;
use sideseat_query_sql::stats::{self, BucketWindow};

use super::query::bind_analytics_values;
use crate::ClickhouseError;

#[derive(Row, Deserialize)]
struct MainAggregationRow {
    traces: u64,
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

#[derive(Row, Deserialize)]
struct CountRow {
    count: u64,
}

#[derive(Row, Deserialize)]
struct AverageRow {
    avg_duration_ms: f64,
}

#[derive(Row, Deserialize)]
struct FrameworkRow {
    framework: Option<String>,
    count: u64,
    percentage: f64,
}

#[derive(Row, Deserialize)]
struct ModelRow {
    model: Option<String>,
    tokens: i64,
    cost: f64,
    percentage: f64,
}

#[derive(Row, Deserialize)]
struct TokenTrendRow {
    bucket: i64,
    tokens: i64,
}

#[derive(Row, Deserialize)]
struct LatencyTrendRow {
    bucket: i64,
    avg_duration_ms: f64,
}

/// Get project stats for the given time range.
pub async fn get_project_stats(
    client: &Client,
    params: &StatsParams,
    now: DateTime<Utc>,
) -> Result<ProjectStatsResult, ClickhouseError> {
    let daily = params.to_timestamp - params.from_timestamp > Duration::hours(48);
    let buckets = calculate_bucket_windows(
        params.from_timestamp,
        params.to_timestamp,
        params.timezone,
        daily,
    );
    let plan = stats::project_stats(params, now, &buckets, Backend::Clickhouse);
    let main: MainAggregationRow = fetch_one(client, &plan.main).await?;

    Ok(ProjectStatsResult {
        counts: CountsResult {
            traces: main.traces as i64,
            traces_previous: query_count(client, &plan.previous_traces).await?,
            sessions: query_count(client, &plan.canonical_sessions).await?,
            spans: main.spans as i64,
            unique_users: main.unique_users as i64,
        },
        costs: CostsResult {
            input: main.input_cost,
            output: main.output_cost,
            cache_read: main.cache_read_cost,
            cache_write: main.cache_write_cost,
            reasoning: main.reasoning_cost,
            total: main.total_cost,
        },
        tokens: TokensResult {
            input: main.input_tokens,
            output: main.output_tokens,
            total: main.total_tokens,
            cache_read: main.cache_read_tokens,
            cache_write: main.cache_write_tokens,
            reasoning: main.reasoning_tokens,
        },
        by_framework: fetch_all::<FrameworkRow>(client, &plan.frameworks)
            .await?
            .into_iter()
            .map(|row| FrameworkBreakdown {
                framework: row.framework,
                count: row.count as i64,
                percentage: row.percentage,
            })
            .collect(),
        by_model: fetch_all::<ModelRow>(client, &plan.models)
            .await?
            .into_iter()
            .map(|row| ModelBreakdown {
                model: row.model,
                tokens: row.tokens,
                cost: row.cost,
                percentage: row.percentage,
            })
            .collect(),
        recent_activity_count: query_count(client, &plan.recent_activity).await?,
        avg_trace_duration_ms: Some(
            fetch_one::<AverageRow>(client, &plan.average_trace_duration)
                .await?
                .avg_duration_ms,
        ),
        trend_data: match &plan.token_trend {
            Some(query) => fetch_all::<TokenTrendRow>(client, query)
                .await?
                .into_iter()
                .map(|row| TrendBucket {
                    bucket: datetime_from_micros(row.bucket),
                    tokens: row.tokens,
                })
                .collect(),
            None => Vec::new(),
        },
        latency_trend_data: match &plan.latency_trend {
            Some(query) => fetch_all::<LatencyTrendRow>(client, query)
                .await?
                .into_iter()
                .map(|row| LatencyBucket {
                    bucket: datetime_from_micros(row.bucket),
                    avg_duration_ms: row.avg_duration_ms,
                })
                .collect(),
            None => Vec::new(),
        },
    })
}

async fn query_count(client: &Client, query: &ParameterizedQuery) -> Result<i64, ClickhouseError> {
    Ok(fetch_one::<CountRow>(client, query).await?.count as i64)
}

async fn fetch_one<T>(client: &Client, query: &ParameterizedQuery) -> Result<T, ClickhouseError>
where
    T: RowOwned + RowRead,
{
    Ok(
        bind_analytics_values(client.query(query.sql()), query.params())
            .fetch_one()
            .await?,
    )
}

async fn fetch_all<T>(
    client: &Client,
    query: &ParameterizedQuery,
) -> Result<Vec<T>, ClickhouseError>
where
    T: RowOwned + RowRead,
{
    Ok(
        bind_analytics_values(client.query(query.sql()), query.params())
            .fetch_all()
            .await?,
    )
}

fn datetime_from_micros(value: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(value).unwrap_or(DateTime::UNIX_EPOCH)
}

fn calculate_bucket_windows(
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    timezone: Tz,
    daily: bool,
) -> Vec<BucketWindow> {
    let local_from = from.with_timezone(&timezone);
    let local_to = to.with_timezone(&timezone);
    let truncated = if daily {
        local_from
            .date_naive()
            .and_time(NaiveTime::from_hms_opt(0, 0, 0).expect("midnight is valid"))
    } else {
        local_from
            .date_naive()
            .and_hms_opt(local_from.hour(), 0, 0)
            .expect("whole hour is valid")
    };
    let step = if daily {
        Duration::days(1)
    } else {
        Duration::hours(1)
    };
    let mut current = match timezone.from_local_datetime(&truncated) {
        chrono::LocalResult::Single(value) => value,
        chrono::LocalResult::Ambiguous(earliest, _) => earliest,
        chrono::LocalResult::None => match timezone.from_local_datetime(&(truncated + step)) {
            chrono::LocalResult::Single(value) => value,
            chrono::LocalResult::Ambiguous(earliest, _) => earliest,
            chrono::LocalResult::None => return Vec::new(),
        },
    };
    let mut boundaries = Vec::new();
    while current <= local_to {
        boundaries.push(current.with_timezone(&Utc));
        current += step;
    }
    boundaries
        .iter()
        .enumerate()
        .map(|(index, start)| BucketWindow {
            start: *start,
            end: boundaries.get(index + 1).copied().unwrap_or(*start + step),
        })
        .collect()
}
