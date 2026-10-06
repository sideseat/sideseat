//! The raw span, its events and its links, rendered from the stored raw record at read time.
//!
//! These used to be a `raw_span` JSON column: the whole OTLP span, serialized again beside the record it came
//! from, about 97 bytes per span after compression, and a second copy of content the authority already held.
//! Redundancy is a defect, so the column is gone and a reader renders the same JSON from the record instead.
//!
//! The rendering is the ingest path's, run backwards: decode the record (splicing its media back from the file
//! store), rebuild each span's JSON with the same builder ingestion used, and run the same file extraction, which
//! re-derives the same `#!B64!#` references. `the_rendered_raw_span_equals_what_ingestion_built` pins the two
//! together over every captured fixture, so the column's answer and this one cannot drift.
//!
//! Only a reader that asks pays: `?include_raw_span=true` and the MCP `get_raw_span` tool. A span list does not,
//! and the event and link counts a list shows are columns.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use sideseat_domain::files::FileService;
use sideseat_domain::raw_payload::{self, RawContent};
use sideseat_ports::traits::RawStore;
use sideseat_ports::types::ProjectId;

use super::persist::build_raw_span_json;
use crate::otlp::extract_attributes;

/// The OTLP JSON of each requested span, keyed by `(trace_id, span_id)`, rendered from the record it was
/// received in - media replaced by `#!B64!#` references exactly as ingestion replaced it.
///
/// A span whose record is gone, unreadable, or missing media is absent from the result rather than guessed at:
/// the caller shows what it has, as it did when the column was null.
pub async fn render_spans(
    project_id: &ProjectId,
    spans: &[(String, String)],
    analytics: &dyn RawStore,
    files: &FileService,
) -> HashMap<(String, String), String> {
    let mut rendered = HashMap::new();
    if spans.is_empty() {
        return rendered;
    }
    let by_span = match analytics.span_raw_ids(project_id, spans).await {
        Ok(found) => found,
        Err(error) => {
            tracing::warn!(%error, "Could not find the raw records of these spans");
            return rendered;
        }
    };
    let mut raw_ids: Vec<String> = by_span.values().cloned().collect();
    raw_ids.sort_unstable();
    raw_ids.dedup();
    let records = match analytics.get_raw_records(project_id, &raw_ids).await {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(%error, "Could not read the raw records of these spans");
            return rendered;
        }
    };
    // Only the spans that were asked for, so one record shared by a thousand spans renders the few wanted.
    let wanted: HashSet<(String, String)> = spans.iter().cloned().collect();
    for record in &records {
        let Some(request) = request_of(project_id, &record.record, files).await else {
            continue;
        };
        rendered.extend(render(&request, Some(&wanted), files.is_enabled()));
    }
    rendered
}

/// Render an export's spans, with no store involved: the part the corpus equality test pins.
pub fn render(
    request: &ExportTraceServiceRequest,
    wanted: Option<&HashSet<(String, String)>>,
    files_enabled: bool,
) -> HashMap<(String, String), String> {
    let mut rendered = HashMap::new();
    for (resource, span) in spans_of(request) {
        let identity = (hex::encode(&span.trace_id), hex::encode(&span.span_id));
        if wanted.is_some_and(|wanted| !wanted.contains(&identity)) {
            continue;
        }
        let mut json = build_raw_span_json(span, &resource);
        // The same replacement ingestion applied to the column, so a reference reads identically.
        if files_enabled {
            let _ = crate::traces::extract::files::extract_and_replace_files(&mut json);
        }
        rendered.insert(
            identity,
            serde_json::to_string(&json).expect("JsonValue is always valid JSON"),
        );
    }
    rendered
}

/// The record's export, with its media spliced back from the file store.
async fn request_of(
    project_id: &ProjectId,
    record: &[u8],
    files: &FileService,
) -> Option<ExportTraceServiceRequest> {
    let hashes = match raw_payload::media_hashes(record) {
        Ok(hashes) => hashes,
        Err(error) => {
            tracing::warn!(%error, "A stored raw record is not readable");
            return None;
        }
    };
    let mut media = HashMap::new();
    for hash in hashes {
        match files.get_file(project_id, &hex::encode(hash)).await {
            Ok(file) => {
                media.insert(hash, file.data.to_vec());
            }
            Err(error) => {
                tracing::warn!(%error, "A raw record's media is not available; not rendering it");
                return None;
            }
        }
    }
    let (content, bytes) = raw_payload::decode(record, |hash| {
        media.get(hash).map(|bytes| Cow::Borrowed(bytes.as_slice()))
    })
    .inspect_err(|error| tracing::warn!(%error, "A stored raw record did not decode"))
    .ok()?;
    match content {
        RawContent::Protobuf => prost::Message::decode(bytes.as_slice()).ok(),
        RawContent::Json => serde_json::from_slice(&bytes).ok(),
    }
}

/// Every span of the export with its resource's attributes, as the ingest path pairs them.
fn spans_of(
    request: &ExportTraceServiceRequest,
) -> Vec<(
    HashMap<String, String>,
    &opentelemetry_proto::tonic::trace::v1::Span,
)> {
    let mut pairs = Vec::new();
    for resource_spans in &request.resource_spans {
        let attributes = resource_spans
            .resource
            .as_ref()
            .map(|resource| extract_attributes(&resource.attributes))
            .unwrap_or_default();
        for scope_spans in &resource_spans.scope_spans {
            for span in &scope_spans.spans {
                pairs.push((attributes.clone(), span));
            }
        }
    }
    pairs
}
