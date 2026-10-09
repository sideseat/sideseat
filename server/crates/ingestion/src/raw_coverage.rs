//! Which spans the raw authority actually holds.
//!
//! The stored rows are a cache of the raw records, so a span is stored only when the record its winning row
//! names exists and holds it. Rows alone can say otherwise: a write can commit its rows and then fail to repair
//! the record to hold them. Two decisions turn on the difference - whether a staged export may be settled, and
//! whether a redelivery of rows already in place may be skipped - and both ask here.

use std::collections::{BTreeMap, HashSet};

use sideseat_ports::error::DataError;
use sideseat_ports::traits::AnalyticsRepository;
use sideseat_ports::types::ProjectId;

use crate::raw_identities::record_identities;
use crate::received::ReceivedPayload;

/// Records read at once. A redelivery names whichever exports first carried its spans, so the records can be many
/// and large; none needs to outlive its own check, and reading them a few at a time bounds what is held to a few
/// records whatever the redelivery names.
const RECORDS_PER_READ: usize = 8;

/// The subset of `spans` - `(trace id, span id)`, hex-encoded as rows store them - held by the record each
/// one's winning row names. An unreadable record holds nothing.
pub(crate) async fn covered(
    analytics: &(dyn AnalyticsRepository + Send + Sync),
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
    let raw_ids: Vec<String> = wanted.keys().map(|raw_id| (*raw_id).to_string()).collect();
    let mut covered = HashSet::new();
    for chunk in raw_ids.chunks(RECORDS_PER_READ) {
        for record in analytics.get_raw_records(project_id, chunk).await? {
            let Ok(held) = record_identities(&record.record) else {
                continue;
            };
            for span in wanted.get(record.raw_id.as_str()).into_iter().flatten() {
                if held.contains(*span) {
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
    project_id: &ProjectId,
    received: &ReceivedPayload,
    spans: &[(String, String)],
) -> Result<HashSet<(String, String)>, DataError> {
    if spans.is_empty() {
        return Ok(HashSet::new());
    }
    let raw_id = crate::traces::raw_id(project_id.as_str(), received);
    let mut covered = HashSet::new();
    for record in analytics.get_raw_records(project_id, &[raw_id]).await? {
        let Ok(held) = record_identities(&record.record) else {
            continue;
        };
        covered.extend(spans.iter().filter(|span| held.contains(*span)).cloned());
    }
    Ok(covered)
}
