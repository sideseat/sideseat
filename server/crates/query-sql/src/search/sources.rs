//! Reading a search page's source text, a chunk of its candidates at a time.

use sideseat_ports::types::SearchSignal;

use super::{Shape, instants};
use crate::Backend;
use crate::analytics::{ParameterizedQuery, QueryValue};

/// The source text of `keys`' revisions, read in one statement, with each row's identity and revision for the
/// caller to keep only the key's own. DuckDB reads the rows it found by row id - a number it can reuse once its row
/// is gone, so the revision is checked too - and only winners. ClickHouse first narrows the identities to those
/// whose winner is still the key's revision, without reading any text, then reads the text of those alone: a
/// replacement written since the page was read is neither fetched nor answered. Either way a revision that is gone
/// is simply absent.
pub fn sources(
    project_id: &str,
    signal: SearchSignal,
    keys: &[sideseat_ports::types::SearchSourceKey],
    backend: Backend,
) -> Option<ParameterizedQuery> {
    if keys.is_empty() {
        return None;
    }
    let shape = Shape::new(signal, backend);
    let projection = shape.source_projection();
    let identity = shape.identity_projection();
    let revision = shape.ingested_micros("r");
    let digest = shape.content_digest("r");
    let table = match signal {
        SearchSignal::Spans => "otel_spans",
        SearchSignal::Logs => "otel_logs",
    };
    let mut params = Vec::new();
    let sql = match backend {
        Backend::Duckdb => {
            let rows: Vec<i64> = keys.iter().filter_map(|key| key.row_id).collect();
            if rows.is_empty() {
                return None;
            }
            // Found through what keeps the read to the rows named - a span's id through its index, a log's own
            // instant through the zone maps - and the row numbers checked after: a scan filtered on the row
            // number alone reads every row of the table.
            let source = match signal {
                SearchSignal::Spans => {
                    let span_ids =
                        crate::keyed::distinct_keys(keys.iter().filter_map(|key| match &key.id {
                            sideseat_ports::types::SearchRecordId::Span { span_id, .. } => {
                                Some(span_id.as_str())
                            }
                            sideseat_ports::types::SearchRecordId::Log { .. } => None,
                        }));
                    params.extend(
                        span_ids
                            .iter()
                            .map(|span_id| QueryValue::String((*span_id).to_string())),
                    );
                    crate::keyed::duckdb_keyed(
                        table,
                        "span_id",
                        &format!(
                            "project_id, trace_id, span_id, superseded_at, ingested_at, content_digest, {}",
                            projection.replace("r.", "")
                        ),
                        span_ids.len(),
                    )
                }
                SearchSignal::Logs => {
                    let at: Vec<i64> = keys.iter().map(|key| key.timestamp_us).collect();
                    format!(
                        "(SELECT *, rowid AS keyed_rowid FROM {table} WHERE \"timestamp\" IN ({}))",
                        instants(&at)
                    )
                }
            };
            params.push(QueryValue::String(project_id.to_string()));
            params.extend(rows.iter().map(|row| QueryValue::Int64(*row)));
            let winner = match signal {
                SearchSignal::Spans => " AND r.superseded_at IS NULL",
                SearchSignal::Logs => "",
            };
            format!(
                "SELECT r.keyed_rowid AS row_id, {identity}, {revision} AS revision_us, \
                 {digest} AS content_digest, {projection} FROM {source} r \
                 WHERE r.project_id = ?{winner} AND r.keyed_rowid IN ({})",
                vec!["?"; rows.len()].join(", ")
            )
        }
        Backend::Clickhouse => {
            let identity_values = |key: &sideseat_ports::types::SearchSourceKey| match &key.id {
                sideseat_ports::types::SearchRecordId::Span { trace_id, span_id } => [
                    QueryValue::String(trace_id.clone()),
                    QueryValue::String(span_id.clone()),
                ],
                sideseat_ports::types::SearchRecordId::Log {
                    log_digest,
                    ordinal,
                } => [
                    QueryValue::String(log_digest.clone()),
                    QueryValue::Int64(i64::from(*ordinal)),
                ],
            };
            params.push(QueryValue::String(project_id.to_string()));
            params.push(QueryValue::String(project_id.to_string()));
            for key in keys {
                params.extend(identity_values(key));
            }
            for key in keys {
                params.extend(identity_values(key));
                params.push(QueryValue::Int64(key.revision_us));
                params.push(QueryValue::String(key.content_digest.clone()));
            }
            let columns = shape.identity_columns();
            // `GLOBAL IN`: the tables are `Distributed` on a cluster, where a plain `IN` over one inside a read of
            // the other is refused (`distributed_product_mode = deny`).
            format!(
                "SELECT {identity}, {revision} AS revision_us, {digest} AS content_digest, {projection} \
                 FROM {table} AS r FINAL WHERE r.project_id = ? AND ({columns}) GLOBAL IN ( \
                 SELECT {columns} FROM (SELECT {columns}, {inner_revision} AS revision_us, \
                 {inner_digest} AS content_digest FROM {table} AS s FINAL \
                 WHERE s.project_id = ? AND ({columns}) IN ({pairs})) \
                 WHERE ({columns}, revision_us, content_digest) IN ({quads}))",
                inner_revision = shape.ingested_micros("s"),
                inner_digest = shape.content_digest("s"),
                pairs = vec!["(?, ?)"; keys.len()].join(", "),
                quads = vec!["(?, ?, ?, ?)"; keys.len()].join(", "),
            )
        }
    };
    Some(ParameterizedQuery::new(sql, params))
}
