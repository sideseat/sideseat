//! Which spans the raw authority actually holds.
//!
//! The stored rows are a cache of the raw records, so a span is stored only when the record its winning row
//! names exists and holds it. Rows alone can say otherwise: a write can commit its rows and then fail to repair
//! the record to hold them. Two decisions turn on the difference - whether a staged export may be settled, and
//! whether a redelivery of rows already in place may be skipped - and both ask here.

use std::collections::{HashMap, HashSet};

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use sideseat_domain::raw_payload::{self, RawContent};
use sideseat_ports::error::DataError;
use sideseat_ports::traits::AnalyticsRepository;
use sideseat_ports::types::ProjectId;

/// The subset of `spans` - `(trace id, span id)`, hex-encoded as rows store them - held by the record each
/// one's winning row names.
pub(crate) async fn covered(
    analytics: &(dyn AnalyticsRepository + Send + Sync),
    project_id: &ProjectId,
    spans: &[(String, String)],
) -> Result<HashSet<(String, String)>, DataError> {
    if spans.is_empty() {
        return Ok(HashSet::new());
    }
    let named = analytics.span_raw_ids(project_id, spans).await?;
    let mut raw_ids: Vec<String> = named.values().cloned().collect();
    raw_ids.sort();
    raw_ids.dedup();
    let records = analytics.get_raw_records(project_id, &raw_ids).await?;
    let held: HashMap<&str, HashSet<(String, String)>> = records
        .iter()
        .map(|record| (record.raw_id.as_str(), spans_held(&record.record)))
        .collect();
    Ok(spans
        .iter()
        .filter(|span| {
            named
                .get(*span)
                .and_then(|raw_id| held.get(raw_id.as_str()))
                .is_some_and(|holds| holds.contains(*span))
        })
        .cloned()
        .collect())
}

/// The `(trace id, span id)` pairs a raw record holds, read through [`raw_payload::decode_shape`], which keeps
/// the record's framing without fetching its media. An unreadable record holds nothing.
fn spans_held(record: &[u8]) -> HashSet<(String, String)> {
    let Ok((content, bytes)) = raw_payload::decode_shape(record) else {
        return HashSet::new();
    };
    let request: Option<ExportTraceServiceRequest> = match content {
        RawContent::Protobuf => ExportTraceServiceRequest::decode(bytes.as_slice()).ok(),
        RawContent::Json => serde_json::from_slice(&bytes).ok(),
    };
    request
        .iter()
        .flat_map(|request| &request.resource_spans)
        .flat_map(|rs| &rs.scope_spans)
        .flat_map(|ss| &ss.spans)
        .map(|span| (hex::encode(&span.trace_id), hex::encode(&span.span_id)))
        .collect()
}
