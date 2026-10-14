//! Staging confirmation's span reads: whether a payload's spans are stored as staged, which are, and each identity's
//! winning revision - each a chunk of identities at a time.

use std::collections::{HashMap, HashSet};

use clickhouse::Client;
use sideseat_ports::types::WinningRevision;
use sideseat_query_sql::keyed::KEYED_CHUNK;
use sideseat_query_sql::{Backend, confirmations};

use super::bind_analytics_values;
use crate::ClickhouseError;

pub async fn spans_match_content(
    client: &Client,
    project_id: &str,
    records: &[(String, String, String)],
) -> Result<bool, ClickhouseError> {
    // A chunk at a time, for `metric::winners`'s reason; every chunk must find all of its records.
    for chunk in sideseat_query_sql::keyed::bounded_chunks(records, KEYED_CHUNK) {
        let Some(plan) = confirmations::spans(project_id, chunk, Backend::Clickhouse) else {
            continue;
        };
        let found: u64 = bind_analytics_values(client.query(plan.query.sql()), plan.query.params())
            .fetch_one()
            .await?;
        if found != plan.expected {
            return Ok(false);
        }
    }
    Ok(true)
}

pub async fn spans_with_matching_content(
    client: &Client,
    project_id: &str,
    records: &[(String, String, String)],
) -> Result<HashSet<(String, String, String)>, ClickhouseError> {
    let mut matching = HashSet::with_capacity(records.len());
    for chunk in sideseat_query_sql::keyed::bounded_chunks(records, KEYED_CHUNK) {
        let Some(query) = confirmations::matching_spans(project_id, chunk, Backend::Clickhouse)
        else {
            continue;
        };
        let rows: Vec<(String, String, String)> =
            bind_analytics_values(client.query(query.sql()), query.params())
                .fetch_all()
                .await?;
        matching.extend(rows);
    }
    Ok(matching)
}

/// The winning revision of each of these span identities that has one.
pub async fn span_winners(
    client: &Client,
    project_id: &str,
    spans: &[(String, String)],
) -> Result<HashMap<(String, String), WinningRevision>, ClickhouseError> {
    let mut rows: Vec<(String, String, String, i64)> = Vec::with_capacity(spans.len());
    for chunk in sideseat_query_sql::keyed::bounded_chunks(spans, KEYED_CHUNK) {
        let Some(query) = confirmations::span_winners(project_id, chunk, Backend::Clickhouse)
        else {
            continue;
        };
        rows.extend(
            bind_analytics_values(client.query(query.sql()), query.params())
                .fetch_all::<(String, String, String, i64)>()
                .await?,
        );
    }
    Ok(rows
        .into_iter()
        .map(|(trace_id, span_id, content_digest, ingested_us)| {
            (
                (trace_id, span_id),
                WinningRevision {
                    content_digest,
                    ingested_at: sideseat_core::utils::time::micros_to_datetime(ingested_us),
                },
            )
        })
        .collect())
}
