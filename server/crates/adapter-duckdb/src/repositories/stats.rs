//! DuckDB execution and decoding for the shared project-statistics plan.

use chrono::{DateTime, Duration, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use duckdb::Connection;
use sideseat_ports::types::{
    CostsResult, CountsResult, FrameworkBreakdown, LatencyBucket, ModelBreakdown,
    ProjectStatsResult, StatsParams, TokensResult, TrendBucket,
};
use sideseat_query_sql::Backend;
use sideseat_query_sql::analytics::{ParameterizedQuery, QueryValue};
use sideseat_query_sql::stats::{self, BucketWindow};

use crate::error::DuckdbError;

/// Get project stats for the given time range.
pub fn get_project_stats(
    conn: &Connection,
    params: &StatsParams,
    now: DateTime<Utc>,
) -> Result<ProjectStatsResult, DuckdbError> {
    let daily = params.to_timestamp - params.from_timestamp > Duration::hours(48);
    let buckets = calculate_bucket_windows(
        params.from_timestamp,
        params.to_timestamp,
        parse_timezone(params.timezone.as_deref()),
        daily,
    );
    let plan = stats::project_stats(params, now, &buckets, Backend::Duckdb);
    let main = query_main(conn, &plan.main)?;

    Ok(ProjectStatsResult {
        counts: CountsResult {
            traces: main.traces,
            traces_previous: query_count(conn, &plan.previous_traces)?,
            sessions: query_count(conn, &plan.canonical_sessions)?,
            spans: main.spans,
            unique_users: main.unique_users,
        },
        costs: main.costs,
        tokens: main.tokens,
        by_framework: query_frameworks(conn, &plan.frameworks)?,
        by_model: query_models(conn, &plan.models)?,
        recent_activity_count: query_count(conn, &plan.recent_activity)?,
        avg_trace_duration_ms: query_average(conn, &plan.average_trace_duration)?,
        trend_data: match &plan.token_trend {
            Some(query) => query_token_trend(conn, query)?,
            None => Vec::new(),
        },
        latency_trend_data: match &plan.latency_trend {
            Some(query) => query_latency_trend(conn, query)?,
            None => Vec::new(),
        },
    })
}

struct MainAggregation {
    traces: i64,
    spans: i64,
    unique_users: i64,
    costs: CostsResult,
    tokens: TokensResult,
}

fn query_main(
    conn: &Connection,
    query: &ParameterizedQuery,
) -> Result<MainAggregation, DuckdbError> {
    let values = duckdb_values(query.params());
    conn.query_row(query.sql(), values.as_slice(), |row| {
        Ok(MainAggregation {
            traces: row.get(0)?,
            spans: row.get(1)?,
            unique_users: row.get(2)?,
            tokens: TokensResult {
                input: row.get(3)?,
                output: row.get(4)?,
                total: row.get(5)?,
                cache_read: row.get(6)?,
                cache_write: row.get(7)?,
                reasoning: row.get(8)?,
            },
            costs: CostsResult {
                input: row.get(9)?,
                output: row.get(10)?,
                cache_read: row.get(11)?,
                cache_write: row.get(12)?,
                reasoning: row.get(13)?,
                total: row.get(14)?,
            },
        })
    })
    .map_err(Into::into)
}

fn query_count(conn: &Connection, query: &ParameterizedQuery) -> Result<i64, DuckdbError> {
    let values = duckdb_values(query.params());
    conn.query_row(query.sql(), values.as_slice(), |row| row.get(0))
        .map_err(Into::into)
}

fn query_average(
    conn: &Connection,
    query: &ParameterizedQuery,
) -> Result<Option<f64>, DuckdbError> {
    let values = duckdb_values(query.params());
    conn.query_row(query.sql(), values.as_slice(), |row| row.get(0))
        .map_err(Into::into)
}

fn query_frameworks(
    conn: &Connection,
    query: &ParameterizedQuery,
) -> Result<Vec<FrameworkBreakdown>, DuckdbError> {
    let values = duckdb_values(query.params());
    let mut statement = conn.prepare(query.sql())?;
    let rows = statement.query_map(values.as_slice(), |row| {
        Ok(FrameworkBreakdown {
            framework: row.get(0)?,
            count: row.get(1)?,
            percentage: row.get(2)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn query_models(
    conn: &Connection,
    query: &ParameterizedQuery,
) -> Result<Vec<ModelBreakdown>, DuckdbError> {
    let values = duckdb_values(query.params());
    let mut statement = conn.prepare(query.sql())?;
    let rows = statement.query_map(values.as_slice(), |row| {
        Ok(ModelBreakdown {
            model: row.get(0)?,
            tokens: row.get(1)?,
            cost: row.get(2)?,
            percentage: row.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn query_token_trend(
    conn: &Connection,
    query: &ParameterizedQuery,
) -> Result<Vec<TrendBucket>, DuckdbError> {
    let values = duckdb_values(query.params());
    let mut statement = conn.prepare(query.sql())?;
    let rows = statement.query_map(values.as_slice(), |row| {
        Ok(TrendBucket {
            bucket: datetime_from_micros(row.get(0)?),
            tokens: row.get(1)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn query_latency_trend(
    conn: &Connection,
    query: &ParameterizedQuery,
) -> Result<Vec<LatencyBucket>, DuckdbError> {
    let values = duckdb_values(query.params());
    let mut statement = conn.prepare(query.sql())?;
    let rows = statement.query_map(values.as_slice(), |row| {
        Ok(LatencyBucket {
            bucket: datetime_from_micros(row.get(0)?),
            avg_duration_ms: row.get(1)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn duckdb_values(values: &[QueryValue]) -> Vec<&dyn duckdb::ToSql> {
    values
        .iter()
        .map(|value| match value {
            QueryValue::String(value) => value as &dyn duckdb::ToSql,
            QueryValue::Int64(value) => value as &dyn duckdb::ToSql,
            QueryValue::Float64(value) => value as &dyn duckdb::ToSql,
        })
        .collect()
}

fn datetime_from_micros(value: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(value).unwrap_or(DateTime::UNIX_EPOCH)
}

fn parse_timezone(timezone: Option<&str>) -> Tz {
    timezone
        .and_then(|value| value.parse::<Tz>().ok())
        .unwrap_or(chrono_tz::UTC)
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn invalid_timezone_defaults_to_utc() {
        assert_eq!(parse_timezone(None), chrono_tz::UTC);
        assert_eq!(parse_timezone(Some("Invalid/Zone")), chrono_tz::UTC);
        assert_eq!(parse_timezone(Some("'; DROP TABLE")), chrono_tz::UTC);
    }

    #[test]
    fn hourly_windows_follow_the_requested_timezone() {
        let from = DateTime::parse_from_rfc3339("2024-01-17T02:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2024-01-17T05:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let windows = calculate_bucket_windows(from, to, chrono_tz::Europe::Berlin, false);
        assert_eq!(windows.len(), 4);
        assert_eq!(windows[0].start.hour(), 2);
        assert_eq!(windows[3].start.hour(), 5);
    }

    #[test]
    fn daily_windows_preserve_local_midnight() {
        let from = DateTime::parse_from_rfc3339("2024-01-16T23:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let to = DateTime::parse_from_rfc3339("2024-01-17T23:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let windows = calculate_bucket_windows(from, to, chrono_tz::Europe::Berlin, true);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].start.day(), 16);
        assert_eq!(windows[0].start.hour(), 23);
        assert_eq!(windows[1].start.day(), 17);
        assert_eq!(windows[1].start.hour(), 23);
    }
}
