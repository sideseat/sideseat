//! Which spans the raw authority actually holds.
//!
//! The stored rows are a cache of the raw records, so a span is stored only when the record its winning row
//! names is live, holds it, and is owned through the span's trace. Rows alone can say otherwise: a write can
//! commit its rows and then fail to repair the record to hold them; a record can be stored but keyed under a
//! collection that is purging it (`RawRecordOwnership.tla`, `LiveStore`); and a record no trace owns is one the
//! next pass may collect. Two decisions turn on the difference - whether a staged export may be settled, and
//! whether a redelivery of rows already in place may be skipped - and both ask here.

use std::collections::{BTreeMap, HashMap, HashSet};

use sideseat_ports::error::DataError;
use sideseat_ports::traits::{AnalyticsRepository, RawArbiter};
use sideseat_ports::types::{
    ProjectId, RawCutoff, RawRecordRow, raw_record_key, raw_trace_key, raw_version_epoch,
};

use crate::raw_identities::record_identities;
use crate::received::ReceivedPayload;

/// Records read at once. A redelivery names whichever exports first carried its spans, so the records can be many
/// and large; none needs to outlive its own check, and reading them a few at a time bounds what is held to a few
/// records whatever the redelivery names.
const RECORDS_PER_READ: usize = 8;

/// The records among `raw_ids` that are live - stored, with their latest keyed above their cutoff.
async fn live_records(
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    database: &(dyn RawArbiter + Send + Sync),
    project_id: &ProjectId,
    raw_ids: &[String],
) -> Result<Vec<RawRecordRow>, DataError> {
    let cutoffs: HashMap<String, RawCutoff> =
        database.raw_record_cutoffs(project_id, raw_ids).await?;
    Ok(analytics
        .get_raw_records(project_id, raw_ids)
        .await?
        .into_iter()
        .filter(|record| {
            cutoffs
                .get(&record.raw_id)
                .is_none_or(|cutoff| raw_version_epoch(record.version) > cutoff.epoch())
        })
        .collect())
}

/// Which of these spans' `(record, trace)` pairs are owners - looked up exactly, so a redelivery's check costs its
/// own spans, never its traces' ownership history.
async fn owners(
    database: &(dyn RawArbiter + Send + Sync),
    project_id: &ProjectId,
    mut keys: Vec<(i64, Vec<u8>)>,
) -> Result<HashSet<(i64, Vec<u8>)>, DataError> {
    keys.sort_unstable();
    keys.dedup();
    Ok(database
        .raw_owned_pairs(project_id, &keys)
        .await?
        .into_iter()
        .collect())
}

/// The subset of `spans` - `(trace id, span id)`, hex-encoded as rows store them - held by the live record
/// each one's winning row names, owned through its trace. An unreadable record holds nothing.
pub(crate) async fn covered(
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    database: &(dyn RawArbiter + Send + Sync),
    project_id: &ProjectId,
    spans: &[(String, String)],
) -> Result<HashSet<(String, String)>, DataError> {
    if spans.is_empty() {
        return Ok(HashSet::new());
    }
    let named = analytics.span_raw_ids(project_id, spans).await?;
    let mut wanted: BTreeMap<&str, Vec<&(String, String)>> = BTreeMap::new();
    for span in spans {
        if let Some(raw_id) = named.get(span) {
            wanted.entry(raw_id.as_str()).or_default().push(span);
        }
    }
    let keys: Vec<(i64, Vec<u8>)> = wanted
        .iter()
        .flat_map(|(raw_id, spans)| {
            spans
                .iter()
                .map(move |(trace_id, _)| (raw_record_key(raw_id), raw_trace_key(trace_id)))
        })
        .collect();
    let owned = owners(database, project_id, keys).await?;
    let raw_ids: Vec<String> = wanted.keys().map(|raw_id| (*raw_id).to_string()).collect();
    let mut covered = HashSet::new();
    for chunk in raw_ids.chunks(RECORDS_PER_READ) {
        for record in live_records(analytics, database, project_id, chunk).await? {
            let Ok(held) = record_identities(&record.record) else {
                continue;
            };
            for span in wanted.get(record.raw_id.as_str()).into_iter().flatten() {
                if held.contains(*span)
                    && owned.contains(&(raw_record_key(&record.raw_id), raw_trace_key(&span.0)))
                {
                    covered.insert((*span).clone());
                }
            }
        }
    }
    Ok(covered)
}

/// The subset of `spans` the export's own record holds - the record `received` is stored as, whatever revision
/// of each span won. An export that is not the winner of a span is stored only when its own record holds it:
/// the winner's record holds the winner's revision, not this one.
pub(crate) async fn held_by_own_record(
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    database: &(dyn RawArbiter + Send + Sync),
    project_id: &ProjectId,
    received: &ReceivedPayload,
    spans: &[(String, String)],
) -> Result<HashSet<(String, String)>, DataError> {
    if spans.is_empty() {
        return Ok(HashSet::new());
    }
    let raw_id = crate::traces::raw_id(project_id.as_str(), received);
    let keys: Vec<(i64, Vec<u8>)> = spans
        .iter()
        .map(|(trace_id, _)| (raw_record_key(&raw_id), raw_trace_key(trace_id)))
        .collect();
    let owned = owners(database, project_id, keys).await?;
    let mut covered = HashSet::new();
    for record in live_records(analytics, database, project_id, &[raw_id]).await? {
        let Ok(held) = record_identities(&record.record) else {
            continue;
        };
        covered.extend(
            spans
                .iter()
                .filter(|span| {
                    held.contains(*span)
                        && owned.contains(&(raw_record_key(&record.raw_id), raw_trace_key(&span.0)))
                })
                .cloned(),
        );
    }
    Ok(covered)
}
