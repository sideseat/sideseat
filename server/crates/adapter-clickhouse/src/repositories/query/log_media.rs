//! The log side of a trace's file references on ClickHouse, which keeps no reference table: a media log's rows are
//! read for the references they hold.

use clickhouse::Client;
use sideseat_query_sql::Backend;

use super::bind_analytics_values;
use crate::ClickhouseError;

/// What the traces' log rows still name: a media log's raw record holds its references.
pub(super) async fn reference_fields_for_traces(
    client: &Client,
    project_id: &str,
    trace_ids: &[String],
    logs: &str,
    fields: &mut Vec<String>,
) -> Result<(), ClickhouseError> {
    if let Some(statement) = sideseat_query_sql::log_media::references_for_traces(
        project_id,
        trace_ids,
        Backend::Clickhouse,
        logs,
    ) {
        let query = bind_analytics_values(client.query(statement.sql()), statement.params());
        let logs: Vec<(String, String, String, String)> = query.fetch_all().await?;
        for (raw_log, body, attributes, messages) in logs {
            // The raw record's values unescaped: SideSeat's references, and the producer's - which ingest verified
            // or noted - where the body that holds them is not stored but rebuilt from the raw record
            // (`domain::log_body`). The derived columns stored beside it are read by their values.
            raw_values(&raw_log, fields);
            for derived in [body, attributes, messages] {
                string_values(&derived, fields);
            }
        }
    }
    Ok(())
}

/// Every string value of a JSON text, keys excluded. Text the parser cannot read - not JSON, or nested deeper than
/// it reads - is taken whole, keys and all: reading too much can only keep a file a key happens to spell for as long
/// as its record lives, while reading too little would release one a value still names.
fn string_values(text: &str, into: &mut Vec<String>) {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) else {
        into.push(text.to_string());
        return;
    };
    let mut pending = vec![parsed];
    while let Some(value) = pending.pop() {
        match value {
            serde_json::Value::String(text) => into.push(text),
            serde_json::Value::Object(members) => {
                pending.extend(members.into_iter().map(|(_, m)| m))
            }
            serde_json::Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
}

/// The OTLP/JSON member naming a key-value pair's key, which is the producer's text and never a reference.
const KEY: &str = "key";

/// Every value of a raw record, unescaped as the producer wrote it (`file_uri::unescape_producer_text`): the strings
/// its views are rebuilt from, so a reference a view shows is among them. A pair's key is not a value. A noted
/// reference is read too - it can only keep a file the trace already owns for as long as the record lives - and text
/// the parser cannot read is taken whole, for the same reason as [`string_values`].
fn raw_values(text: &str, into: &mut Vec<String>) {
    let unescape =
        |text: &str| sideseat_core::utils::file_uri::unescape_producer_text(text).into_owned();
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) else {
        into.push(unescape(text));
        return;
    };
    let mut pending = vec![parsed];
    while let Some(value) = pending.pop() {
        match value {
            serde_json::Value::String(text) => into.push(unescape(&text)),
            serde_json::Value::Object(members) => pending.extend(
                members
                    .into_iter()
                    .filter(|(member, _)| member != KEY)
                    .map(|(_, value)| value),
            ),
            serde_json::Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
}

/// One page of the traces whose stored logs hold media, read across every replica (`logs`).
pub async fn media_log_trace_ids(
    client: &Client,
    project_id: &str,
    after: Option<&str>,
    limit: usize,
    logs: &str,
) -> Result<Vec<String>, ClickhouseError> {
    let query = sideseat_query_sql::log_media::media_trace_page(
        project_id,
        after,
        limit,
        Backend::Clickhouse,
        logs,
    );
    Ok(
        bind_analytics_values(client.query(query.sql()), query.params())
            .fetch_all::<String>()
            .await?,
    )
}
