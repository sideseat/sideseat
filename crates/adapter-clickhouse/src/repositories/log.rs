//! ClickHouse OTLP log writes and typed reads.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use clickhouse::{Client, Row};
use serde::{Deserialize, Serialize};
use sideseat_core::utils::json::json_to_opt_string;
use sideseat_ports::traits::FilterOptionRow;
use sideseat_ports::types::{ListLogsParams, LogRow, NormalizedLog, ProjectId, SearchField};
use sideseat_query_sql::{Backend, confirmations, dml, logs as log_sql};

use super::query::bind_analytics_values;
use crate::ClickhouseError;

#[derive(Row, Serialize)]
struct ChLogInsertRow {
    project_id: String,
    log_digest: String,
    ordinal: u32,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    timestamp: time::OffsetDateTime,
    #[serde(with = "clickhouse::serde::time::datetime64::micros::option")]
    time: Option<time::OffsetDateTime>,
    #[serde(with = "clickhouse::serde::time::datetime64::micros::option")]
    observed_time: Option<time::OffsetDateTime>,
    severity_number: i32,
    severity_text: Option<String>,
    body: Option<String>,
    body_text: Option<String>,
    attributes: Option<String>,
    dropped_attributes_count: u32,
    flags: u32,
    trace_id: Option<String>,
    span_id: Option<String>,
    event_name: Option<String>,
    session_id: Option<String>,
    user_id: Option<String>,
    environment: Option<String>,
    service_name: Option<String>,
    service_version: Option<String>,
    service_namespace: Option<String>,
    service_instance_id: Option<String>,
    resource_attributes: Option<String>,
    scope_name: Option<String>,
    scope_version: Option<String>,
    scope_attributes: Option<String>,
    scope_schema_url: Option<String>,
    resource_schema_url: Option<String>,
    raw_log: Option<String>,
    #[serde(with = "clickhouse::serde::time::datetime64::micros")]
    ingested_at: time::OffsetDateTime,
    #[serde(with = "clickhouse::serde::time::datetime64::micros::option")]
    hold_until: Option<time::OffsetDateTime>,
    logical_bytes: u64,
    search_indexed: u8,
    search_body: Vec<String>,
    search_body_truncated: u8,
    search_event_name: Vec<String>,
    search_event_name_truncated: u8,
    search_severity: Vec<String>,
    search_severity_truncated: u8,
    search_attributes: Vec<String>,
    search_attributes_truncated: u8,
}

impl From<&NormalizedLog> for ChLogInsertRow {
    fn from(log: &NormalizedLog) -> Self {
        Self {
            project_id: log.project_id.clone().unwrap_or_default(),
            log_digest: log.log_digest.clone(),
            ordinal: log.ordinal,
            timestamp: chrono_to_time(log.timestamp),
            time: log.time.map(chrono_to_time),
            observed_time: log.observed_time.map(chrono_to_time),
            severity_number: log.severity_number,
            severity_text: log.severity_text.clone(),
            body: json_to_opt_string(&log.body),
            body_text: log.body_text.clone(),
            attributes: json_to_opt_string(&log.attributes),
            dropped_attributes_count: log.dropped_attributes_count,
            flags: log.flags,
            trace_id: log.trace_id.clone(),
            span_id: log.span_id.clone(),
            event_name: log.event_name.clone(),
            session_id: log.session_id.clone(),
            user_id: log.user_id.clone(),
            environment: log.environment.clone(),
            service_name: log.service_name.clone(),
            service_version: log.service_version.clone(),
            service_namespace: log.service_namespace.clone(),
            service_instance_id: log.service_instance_id.clone(),
            resource_attributes: json_to_opt_string(&log.resource_attributes),
            scope_name: log.scope_name.clone(),
            scope_version: log.scope_version.clone(),
            scope_attributes: json_to_opt_string(&log.scope_attributes),
            scope_schema_url: log.scope_schema_url.clone(),
            resource_schema_url: log.resource_schema_url.clone(),
            raw_log: json_to_opt_string(&log.raw_log),
            ingested_at: chrono_to_time(log.ingested_at.unwrap_or(log.timestamp)),
            hold_until: log.hold_until.map(chrono_to_time),
            logical_bytes: log.logical_bytes,
            search_indexed: u8::from(log.search.indexed),
            search_body: search_terms(log, SearchField::Body).0,
            search_body_truncated: search_terms(log, SearchField::Body).1,
            search_event_name: search_terms(log, SearchField::EventName).0,
            search_event_name_truncated: search_terms(log, SearchField::EventName).1,
            search_severity: search_terms(log, SearchField::Severity).0,
            search_severity_truncated: search_terms(log, SearchField::Severity).1,
            search_attributes: search_terms(log, SearchField::Attributes).0,
            search_attributes_truncated: search_terms(log, SearchField::Attributes).1,
        }
    }
}

fn search_terms(log: &NormalizedLog, field: SearchField) -> (Vec<String>, u8) {
    log.search.field(field).map_or_else(
        || (Vec::new(), 0),
        |entry| (entry.terms.clone(), u8::from(entry.truncated)),
    )
}

#[derive(Row, Deserialize)]
struct ChLogReadRow {
    log_digest: String,
    ordinal: u32,
    timestamp_us: i64,
    time_us: Option<i64>,
    observed_time_us: Option<i64>,
    severity_number: i32,
    severity_text: Option<String>,
    body: Option<String>,
    body_text: Option<String>,
    attributes: Option<String>,
    dropped_attributes_count: u32,
    flags: u32,
    trace_id: Option<String>,
    span_id: Option<String>,
    event_name: Option<String>,
    session_id: Option<String>,
    user_id: Option<String>,
    environment: Option<String>,
    service_name: Option<String>,
    service_version: Option<String>,
    service_namespace: Option<String>,
    service_instance_id: Option<String>,
    resource_attributes: Option<String>,
    scope_name: Option<String>,
    scope_version: Option<String>,
    scope_attributes: Option<String>,
    scope_schema_url: Option<String>,
    resource_schema_url: Option<String>,
    raw_log: Option<String>,
    ingested_at_us: i64,
}

impl From<ChLogReadRow> for LogRow {
    fn from(row: ChLogReadRow) -> Self {
        Self {
            log_digest: row.log_digest,
            ordinal: row.ordinal,
            timestamp: datetime_from_micros(row.timestamp_us),
            time: row.time_us.and_then(DateTime::from_timestamp_micros),
            observed_time: row
                .observed_time_us
                .and_then(DateTime::from_timestamp_micros),
            severity_number: row.severity_number,
            severity_text: row.severity_text,
            body: row.body,
            body_text: row.body_text,
            attributes: row.attributes,
            dropped_attributes_count: row.dropped_attributes_count,
            flags: row.flags,
            trace_id: row.trace_id,
            span_id: row.span_id,
            event_name: row.event_name,
            session_id: row.session_id,
            user_id: row.user_id,
            environment: row.environment,
            service_name: row.service_name,
            service_version: row.service_version,
            service_namespace: row.service_namespace,
            service_instance_id: row.service_instance_id,
            resource_attributes: row.resource_attributes,
            scope_name: row.scope_name,
            scope_version: row.scope_version,
            scope_attributes: row.scope_attributes,
            scope_schema_url: row.scope_schema_url,
            resource_schema_url: row.resource_schema_url,
            raw_log: row.raw_log,
            ingested_at: datetime_from_micros(row.ingested_at_us),
        }
    }
}

#[derive(Row, Deserialize)]
struct ChFilterOptionRow {
    value: Option<String>,
    count: u64,
}

pub async fn insert_batch(
    client: &Client,
    table_name: &str,
    logs: &[NormalizedLog],
) -> Result<(), ClickhouseError> {
    if logs.is_empty() {
        return Ok(());
    }
    let target = dml::log_write_target(Backend::Clickhouse, Some(table_name));
    let mut insert: clickhouse::insert::Insert<ChLogInsertRow> =
        client.insert(target.table()).await?;
    for log in logs {
        insert.write(&ChLogInsertRow::from(log)).await?;
    }
    insert.end().await?;
    Ok(())
}

pub async fn list_logs(
    client: &Client,
    params: &ListLogsParams,
) -> Result<(Vec<LogRow>, u64), ClickhouseError> {
    let page = log_sql::list_logs(params, Backend::Clickhouse);
    let total: u64 = bind_analytics_values(client.query(page.count.sql()), page.count.params())
        .fetch_one()
        .await?;
    let rows: Vec<ChLogReadRow> =
        bind_analytics_values(client.query(page.rows.sql()), page.rows.params())
            .fetch_all()
            .await?;
    Ok((rows.into_iter().map(Into::into).collect(), total))
}

pub async fn get_log(
    client: &Client,
    project_id: &ProjectId,
    log_digest: &str,
    ordinal: u32,
) -> Result<Option<LogRow>, ClickhouseError> {
    let query = log_sql::get_log(
        project_id.as_str(),
        log_digest,
        ordinal,
        Backend::Clickhouse,
    );
    let row: Option<ChLogReadRow> =
        bind_analytics_values(client.query(query.sql()), query.params())
            .fetch_optional()
            .await?;
    Ok(row.map(Into::into))
}

pub async fn matches_content(
    client: &Client,
    project_id: &ProjectId,
    records: &[(String, u32)],
) -> Result<bool, ClickhouseError> {
    let Some(plan) = confirmations::logs(project_id.as_str(), records, Backend::Clickhouse) else {
        return Ok(true);
    };
    let found: u64 = bind_analytics_values(client.query(plan.query.sql()), plan.query.params())
        .fetch_one()
        .await?;
    Ok(found == plan.expected)
}

pub async fn get_log_filter_options(
    client: &Client,
    project_id: &ProjectId,
    columns: &[String],
    from_timestamp: Option<DateTime<Utc>>,
    to_timestamp: Option<DateTime<Utc>>,
) -> Result<HashMap<String, Vec<FilterOptionRow>>, ClickhouseError> {
    let mut result = HashMap::new();
    for option in log_sql::log_filter_options(
        project_id.as_str(),
        columns,
        from_timestamp,
        to_timestamp,
        Backend::Clickhouse,
    ) {
        let rows: Vec<ChFilterOptionRow> =
            bind_analytics_values(client.query(option.query.sql()), option.query.params())
                .fetch_all()
                .await?;
        result.insert(
            option.column,
            rows.into_iter()
                .filter_map(|row| {
                    row.value.map(|value| FilterOptionRow {
                        value,
                        count: row.count,
                    })
                })
                .collect(),
        );
    }
    Ok(result)
}

fn chrono_to_time(value: DateTime<Utc>) -> time::OffsetDateTime {
    let (value, _) = sideseat_core::utils::time::clamp_to_storable(value);
    time::OffsetDateTime::from_unix_timestamp(value.timestamp())
        .map(|time| time + time::Duration::nanoseconds(i64::from(value.timestamp_subsec_nanos())))
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
}

fn datetime_from_micros(value: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(value).unwrap_or(DateTime::UNIX_EPOCH)
}
