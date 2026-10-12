//! DuckDB OTLP log writes and typed reads.

use std::collections::HashMap;

use duckdb::{Connection, params};
use sideseat_core::utils::json::json_to_opt_string;
use sideseat_core::utils::time::micros_to_datetime;
use sideseat_ports::traits::FilterOptionRow;
use sideseat_ports::types::{ListLogsParams, LogRow, NormalizedLog, ProjectId, SearchSignal};
use sideseat_query_sql::analytics::{ParameterizedQuery, QueryValue};
use sideseat_query_sql::{Backend, confirmations, dml, logs as log_sql};

use crate::sql_types::{SqlOptTimestamp, SqlTimestamp};
use crate::{DuckdbError, in_transaction};

pub fn insert_batch(conn: &Connection, logs: &[NormalizedLog]) -> Result<(), DuckdbError> {
    if logs.is_empty() {
        return Ok(());
    }
    let target = dml::log_write_target(Backend::Duckdb, None);
    in_transaction(conn, |conn| {
        // A redelivery replaces the identity's stored row. Found within the records' own instants and deleted by
        // row id, so a write reads the row groups of what it replaces rather than the whole table (`keyed`).
        // A record with a time of its own is stored under it - part of its digest - so its row is found at that
        // instant; one with neither time nor observed time is stored under a receipt time another delivery
        // replaces, and is looked up without one.
        let identities: Vec<(super::keyed::LogIdentity, Option<i64>)> = logs
            .iter()
            .map(|log| {
                (
                    (
                        log.project_id.clone().unwrap_or_default(),
                        log.log_digest.clone(),
                        log.ordinal,
                    ),
                    (log.time.is_some() || log.observed_time.is_some())
                        .then(|| log.timestamp.timestamp_micros()),
                )
            })
            .collect();
        let stored_rows = super::keyed::log_rows(conn, &identities)?;
        let mut stored: HashMap<super::keyed::LogIdentity, Vec<i64>> = HashMap::new();
        let mut rowids = Vec::with_capacity(stored_rows.len());
        for (rowid, identity, ingested_us) in stored_rows {
            rowids.push(rowid);
            stored.entry(identity).or_default().push(ingested_us);
        }
        super::keyed::delete_rows(conn, target.table(), &rowids)?;
        // A replaced identity's reference rows for the references this delivery writes again go, so they are
        // not doubled.
        let rewritten: Vec<(String, String, u32, String)> = logs
            .iter()
            .filter(|log| {
                stored.contains_key(&(
                    log.project_id.clone().unwrap_or_default(),
                    log.log_digest.clone(),
                    log.ordinal,
                ))
            })
            .flat_map(|log| {
                log.media.iter().map(|uri| {
                    (
                        log.project_id.clone().unwrap_or_default(),
                        log.log_digest.clone(),
                        log.ordinal,
                        uri.clone(),
                    )
                })
            })
            .collect();
        if let Some(delete) = sideseat_query_sql::log_media::duckdb_delete_rewritten(&rewritten) {
            conn.execute(delete.sql(), query_values(delete.params()).as_slice())?;
        }
        // The replaced rows' other media references stay: a replacement that no longer names a file still leaves the
        // association its earlier delivery made, and only a reference the release can see lets the release run
        // when the record goes. Kept until the record's row is deleted, they hold a file no longer than the record
        // lives; and nothing here reads the reference table, so a write costs what it writes.
        let mut appender = conn.appender(target.table())?;
        for log in logs {
            let (search_fields, search_truncated) =
                sideseat_query_sql::search::duckdb_document_bits(SearchSignal::Logs, &log.search);
            appender.append_row(params![
                log.project_id.as_deref().unwrap_or_default(),
                log.log_digest.as_str(),
                log.ordinal,
                SqlTimestamp(log.timestamp),
                SqlOptTimestamp(log.time),
                SqlOptTimestamp(log.observed_time),
                log.severity_number,
                log.severity_text.as_deref(),
                json_to_opt_string(&log.body).as_deref(),
                log.body_text.as_deref(),
                json_to_opt_string(&log.attributes).as_deref(),
                log.dropped_attributes_count,
                log.flags,
                log.trace_id.as_deref(),
                log.span_id.as_deref(),
                log.event_name.as_deref(),
                log.session_id.as_deref(),
                log.user_id.as_deref(),
                log.environment.as_deref(),
                log.service_name.as_deref(),
                log.service_version.as_deref(),
                log.service_namespace.as_deref(),
                log.service_instance_id.as_deref(),
                json_to_opt_string(&log.resource_attributes).as_deref(),
                log.scope_name.as_deref(),
                log.scope_version.as_deref(),
                json_to_opt_string(&log.scope_attributes).as_deref(),
                log.scope_schema_url.as_deref(),
                log.resource_schema_url.as_deref(),
                json_to_opt_string(&log.raw_log).as_deref(),
                SqlTimestamp(log.ingested_at.unwrap_or(log.timestamp)),
                SqlOptTimestamp(log.hold_until),
                i64::try_from(log.logical_bytes).unwrap_or(i64::MAX),
                log.messages.as_deref().unwrap_or("[]"),
                search_fields,
                search_truncated,
                log.frame_key,
            ])?;
        }
        appender.flush()?;
        drop(appender);
        write_media_refs(conn, logs)?;
        super::search::replace_log_terms(conn, logs, &stored)?;
        Ok(())
    })
}

/// One reference row per media-bearing log and reference (`log_media_refs`). A log without a trace holds its media
/// inline, so it has none.
fn write_media_refs(conn: &Connection, logs: &[NormalizedLog]) -> Result<(), DuckdbError> {
    if logs.iter().all(|log| log.media.is_empty()) {
        return Ok(());
    }
    let mut appender = conn.appender("log_media_refs")?;
    for log in logs {
        let Some(trace_id) = log.trace_id.as_deref() else {
            continue;
        };
        for uri in &log.media {
            appender.append_row(params![
                log.project_id.as_deref().unwrap_or_default(),
                trace_id,
                log.span_id.as_deref(),
                log.log_digest.as_str(),
                log.ordinal,
                uri.as_str(),
            ])?;
        }
    }
    appender.flush()?;
    Ok(())
}

pub fn list_logs(
    conn: &Connection,
    params: &ListLogsParams,
) -> Result<(Vec<LogRow>, u64), DuckdbError> {
    let page = log_sql::list_logs(params, Backend::Duckdb);
    let count_values = query_values(page.count.params());
    let total: i64 = conn.query_row(page.count.sql(), count_values.as_slice(), |row| row.get(0))?;
    Ok((execute_rows(conn, &page.rows)?, total as u64))
}

pub fn matches_content(
    conn: &Connection,
    project_id: &ProjectId,
    records: &[(String, u32, Option<chrono::DateTime<chrono::Utc>>)],
) -> Result<bool, DuckdbError> {
    // Records with an instant of their own are read within their instants; the rest, unbounded, apart.
    let (bounded, unbounded): (Vec<_>, Vec<_>) = records
        .iter()
        .cloned()
        .partition(|(_, _, instant)| instant.is_some());
    for chunk in bounded
        .chunks(sideseat_query_sql::keyed::KEYED_CHUNK)
        .chain(unbounded.chunks(sideseat_query_sql::keyed::KEYED_CHUNK))
    {
        let Some(plan) = confirmations::logs(project_id.as_str(), chunk, Backend::Duckdb) else {
            continue;
        };
        let values = query_values(plan.query.params());
        let found: i64 = conn.query_row(plan.query.sql(), values.as_slice(), |row| row.get(0))?;
        if found as u64 != plan.expected {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn get_log(
    conn: &Connection,
    project_id: &ProjectId,
    log_digest: &str,
    ordinal: u32,
) -> Result<Option<LogRow>, DuckdbError> {
    let query = log_sql::get_log(project_id.as_str(), log_digest, ordinal, Backend::Duckdb);
    Ok(execute_rows(conn, &query)?.into_iter().next())
}

pub fn get_log_filter_options(
    conn: &Connection,
    project_id: &ProjectId,
    columns: &[String],
    from_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    to_timestamp: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<HashMap<String, Vec<FilterOptionRow>>, DuckdbError> {
    let mut result = HashMap::new();
    for option in log_sql::log_filter_options(
        project_id.as_str(),
        columns,
        from_timestamp,
        to_timestamp,
        Backend::Duckdb,
    ) {
        let values = query_values(option.query.params());
        let mut statement = conn.prepare(option.query.sql())?;
        let rows = statement.query_map(values.as_slice(), |row| {
            Ok(FilterOptionRow {
                value: row.get(0)?,
                count: row.get::<_, i64>(1)? as u64,
            })
        })?;
        result.insert(option.column, rows.collect::<Result<Vec<_>, _>>()?);
    }
    Ok(result)
}

fn execute_rows(conn: &Connection, query: &ParameterizedQuery) -> Result<Vec<LogRow>, DuckdbError> {
    let values = query_values(query.params());
    let mut statement = conn.prepare(query.sql())?;
    let rows = statement.query_map(values.as_slice(), |row| {
        Ok(LogRow {
            log_digest: row.get(0)?,
            ordinal: row.get(1)?,
            timestamp: micros_to_datetime(row.get(2)?),
            time: row.get::<_, Option<i64>>(3)?.map(micros_to_datetime),
            observed_time: row.get::<_, Option<i64>>(4)?.map(micros_to_datetime),
            severity_number: row.get(5)?,
            severity_text: row.get(6)?,
            body: row.get(7)?,
            body_text: row.get(8)?,
            attributes: row.get(9)?,
            dropped_attributes_count: row.get(10)?,
            flags: row.get(11)?,
            trace_id: row.get(12)?,
            span_id: row.get(13)?,
            event_name: row.get(14)?,
            session_id: row.get(15)?,
            user_id: row.get(16)?,
            environment: row.get(17)?,
            service_name: row.get(18)?,
            service_version: row.get(19)?,
            service_namespace: row.get(20)?,
            service_instance_id: row.get(21)?,
            resource_attributes: row.get(22)?,
            scope_name: row.get(23)?,
            scope_version: row.get(24)?,
            scope_attributes: row.get(25)?,
            scope_schema_url: row.get(26)?,
            resource_schema_url: row.get(27)?,
            raw_log: row.get(28)?,
            ingested_at: micros_to_datetime(row.get(29)?),
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn query_values(values: &[QueryValue]) -> Vec<&dyn duckdb::ToSql> {
    values
        .iter()
        .map(|value| match value {
            QueryValue::String(value) => value as &dyn duckdb::ToSql,
            QueryValue::Int64(value) => value as &dyn duckdb::ToSql,
            QueryValue::Float64(value) => value as &dyn duckdb::ToSql,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use sideseat_core::storage::AppStorage;
    use tempfile::TempDir;

    async fn service() -> (TempDir, crate::DuckdbService) {
        let directory = TempDir::new().unwrap();
        let storage = AppStorage::init_for_test(directory.path().to_path_buf());
        let service = crate::DuckdbService::init(&storage, std::sync::Arc::new(crate::TestClock))
            .await
            .unwrap();
        (directory, service)
    }

    fn fixture(digest: &str, ordinal: u32, second: i64) -> NormalizedLog {
        let timestamp = DateTime::from_timestamp(1_700_000_000 + second, 0).unwrap();
        NormalizedLog {
            project_id: Some("logs-project".to_string()),
            log_digest: digest.to_string(),
            ordinal,
            timestamp,
            time: Some(timestamp),
            severity_number: 17,
            severity_text: Some("ERROR".to_string()),
            body: serde_json::json!("checkout failed"),
            body_text: Some("checkout failed".to_string()),
            attributes: serde_json::json!({"attempt": 2}),
            dropped_attributes_count: 1,
            flags: 1,
            trace_id: Some("trace-1".to_string()),
            span_id: Some("span-1".to_string()),
            event_name: Some("payment.failed".to_string()),
            service_name: Some("checkout".to_string()),
            environment: Some("test".to_string()),
            resource_attributes: serde_json::json!({"service.name": "checkout"}),
            scope_attributes: serde_json::json!({}),
            raw_log: serde_json::json!({"body": "checkout failed"}),
            ingested_at: Some(timestamp),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn logs_round_trip_with_total_order_correlation_and_retry_dedup() {
        let (_directory, service) = service().await;
        let conn = service.conn();
        let first = fixture("digest-a", 0, 0);
        let second = fixture("digest-b", 0, 1);
        insert_batch(&conn, &[first.clone(), second]).unwrap();
        insert_batch(&conn, &[first]).unwrap();

        let project_id = ProjectId::from("logs-project");
        let params = ListLogsParams {
            project_id: project_id.clone(),
            page: 1,
            limit: 20,
            trace_id: Some("trace-1".to_string()),
            span_id: Some("span-1".to_string()),
            ..Default::default()
        };
        let (rows, total) = list_logs(&conn, &params).unwrap();
        assert_eq!(total, 2, "a byte-identical retry is one identity");
        assert_eq!(rows[0].log_digest, "digest-b");
        assert_eq!(rows[1].body_text.as_deref(), Some("checkout failed"));
        assert_eq!(rows[1].attributes.as_deref(), Some("{\"attempt\":2}"));

        let point = get_log(&conn, &project_id, "digest-a", 0)
            .unwrap()
            .expect("log exists");
        assert_eq!(point.trace_id.as_deref(), Some("trace-1"));
        assert_eq!(point.span_id.as_deref(), Some("span-1"));

        let options = get_log_filter_options(
            &conn,
            &project_id,
            &["severity_text".to_string(), "service_name".to_string()],
            None,
            None,
        )
        .unwrap();
        assert_eq!(options["severity_text"][0].value, "ERROR");
        assert_eq!(options["severity_text"][0].count, 2);
        assert_eq!(options["service_name"][0].value, "checkout");
    }

    /// A redelivery keeps one reference row per reference it writes again, and the reference an earlier delivery
    /// wrote that this one no longer names stays until the record's row goes.
    #[tokio::test]
    async fn a_redelivery_keeps_one_reference_row_each_and_the_earlier_ones() {
        let (_directory, service) = service().await;
        let conn = service.conn();
        let kept = format!("#!B64!#image/png::{}", "a".repeat(64));
        let dropped = format!("#!B64!#image/png::{}", "b".repeat(64));
        let first = NormalizedLog {
            media: vec![kept.clone(), dropped.clone()],
            ..fixture("digest-a", 0, 0)
        };
        insert_batch(&conn, std::slice::from_ref(&first)).unwrap();
        for _ in 0..3 {
            insert_batch(
                &conn,
                &[NormalizedLog {
                    media: vec![kept.clone()],
                    ..first.clone()
                }],
            )
            .unwrap();
        }
        let query = sideseat_query_sql::log_media::references_for_traces(
            "logs-project",
            &["trace-1".to_string()],
            Backend::Duckdb,
            "log_media_refs",
        )
        .expect("a reference read");
        let mut uris = super::super::query::string_column(&conn, &query).unwrap();
        uris.sort();
        let mut rows: Vec<(String, i64)> = Vec::new();
        for uri in uris {
            match rows.last_mut() {
                Some((last, count)) if *last == uri => *count += 1,
                _ => rows.push((uri, 1)),
            }
        }
        assert_eq!(rows, vec![(kept, 1), (dropped, 1)]);
    }
}
