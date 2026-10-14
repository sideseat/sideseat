//! A metric data point's exemplars in DuckDB: one `STRUCT(...)[]` column, every exemplar once.
//!
//! The positional appender binds no list or struct value, and DuckDB's text form of a list cannot carry every
//! string - a value with quotes and backslashes in some orders does not cast back. So a batch is appended to a
//! staging table that holds each row's exemplars as JSON, which `serde_json` writes and DuckDB's `from_json` reads
//! exactly, and moved into the table by one `INSERT ... SELECT` that converts them. Read back, the list arrives as a
//! `Value::List` of `Value::Struct`s.

use duckdb::Connection;
use duckdb::types::Value;
use serde_json::{Value as JsonValue, json};
use sideseat_ports::types::MetricExemplar;

use crate::error::DuckdbError;

/// The staging table a metric batch is appended to: the metric table's columns in its order, with the exemplars
/// as JSON text. Temporary, so it is the connection's own and never part of the store.
pub(crate) const STAGE: &str = "otel_metrics_stage";

/// The JSON shape `from_json` reads the staged exemplars by: the column's struct, field for field.
const STAGED_SHAPE: &str = r#"[{"trace_id":"VARCHAR","span_id":"VARCHAR","time_unix_nano":"UBIGINT","value_int":"BIGINT","value_double":"DOUBLE","attributes":"VARCHAR"}]"#;

/// Make the staging table if this connection has none, and empty it.
pub(crate) fn prepare_stage(conn: &Connection, table: &str) -> Result<(), DuckdbError> {
    conn.execute_batch(&format!(
        "CREATE TEMP TABLE IF NOT EXISTS {STAGE} AS \
         SELECT * REPLACE (NULL::VARCHAR AS exemplars) FROM {table} LIMIT 0; \
         DELETE FROM {STAGE};"
    ))?;
    Ok(())
}

/// Move the staged batch into `table`, its exemplars converted to the list, and empty the stage.
pub(crate) fn commit_stage(conn: &Connection, table: &str) -> Result<(), DuckdbError> {
    conn.execute_batch(&format!(
        "INSERT INTO {table} SELECT * REPLACE (from_json(exemplars, '{STAGED_SHAPE}') AS exemplars) \
         FROM {STAGE}; DELETE FROM {STAGE};"
    ))?;
    Ok(())
}

/// A row's exemplars as the JSON the stage holds; `None` for a point with none.
pub(crate) fn staged_exemplars(exemplars: &[MetricExemplar]) -> Option<String> {
    (!exemplars.is_empty()).then(|| {
        JsonValue::Array(
            exemplars
                .iter()
                .map(|exemplar| {
                    json!({
                        "trace_id": exemplar.trace_id_hex(),
                        "span_id": exemplar.span_id_hex(),
                        "time_unix_nano": exemplar.time_unix_nano,
                        "value_int": exemplar.value_int,
                        "value_double": exemplar.value_double,
                        "attributes": exemplar.attributes,
                    })
                })
                .collect(),
        )
        .to_string()
    })
}

/// A row's exemplars as the column returns them; empty for `NULL`.
pub(crate) fn exemplars_from_value(value: Value) -> Vec<MetricExemplar> {
    let Value::List(items) = value else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| {
            let Value::Struct(fields) = item else {
                return None;
            };
            let mut exemplar = MetricExemplar::default();
            for (name, field) in fields.iter() {
                match (name.as_str(), field) {
                    ("trace_id", Value::Text(hex)) => {
                        exemplar.trace_id = MetricExemplar::id_from_hex(hex)
                    }
                    ("span_id", Value::Text(hex)) => {
                        exemplar.span_id = MetricExemplar::id_from_hex(hex)
                    }
                    ("time_unix_nano", Value::UBigInt(nanos)) => exemplar.time_unix_nano = *nanos,
                    ("value_int", Value::BigInt(value)) => exemplar.value_int = Some(*value),
                    ("value_double", Value::Double(value)) => exemplar.value_double = Some(*value),
                    ("attributes", Value::Text(text)) => exemplar.attributes = Some(text.clone()),
                    _ => {}
                }
            }
            Some(exemplar)
        })
        .collect()
}

#[cfg(test)]
#[path = "metric_exemplars_tests.rs"]
mod tests;
