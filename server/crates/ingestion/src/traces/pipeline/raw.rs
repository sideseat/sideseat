//! The raw record of a trace export: encoded once per request, its media owned by the traces that carry it,
//! and written before the rows derived from it.
//!
//! The record is the received body with its media cut out (`sideseat_domain::raw_payload`). The media objects
//! go through the same pending file writes as the files a span's extraction finds - same content address, same
//! `trace_files` ownership with `pending_writers` and `durable` - so an image a span already extracted is the
//! same object, and retention, deletion, legal hold and restore treat raw media exactly as they treat files.
//! An object belongs to every trace whose span text carries it; one found nowhere in a span (in a resource
//! attribute, say) belongs to every trace of the export, which keeps it at least as long as any of them.
//!
//! Media the file store did not keep - a project over its quota, or file storage disabled - stays inline, so a
//! record is decodable whatever happened to the files.
//!
//! Every version of a record is the received export filtered to a subset of its spans, the shape
//! `server/specs/RawRecordOwnership.tla` relies on: a filtered version is re-encoded from the *received* body,
//! never from the request the pipeline mutated (the injected project attribute, stripped spans).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use prost::Message;
use sideseat_core::constants::FILE_HASH_ALGORITHM;
use sideseat_core::utils::file_uri::parse_file_uri;
use sideseat_core::utils::mime::detect_mime_type_from_base64;
use sideseat_domain::raw_payload::{self, EncodedRaw, RawContent};
use sideseat_ports::types::{ProjectId, RawOrigin, RawRecordRow, StagedSignal};

use super::super::persist::PendingFileWrite;
use crate::received::ReceivedPayload;

/// A span's identity within one project: hex trace id, hex span id.
pub(in crate::traces) type SpanKey = (String, String);

/// A request's raw record before it is stored.
pub(in crate::traces) struct RawDraft {
    project_id: String,
    received: ReceivedPayload,
    encoded: EncodedRaw,
    /// Media left inline because the file store did not keep it.
    inline: HashSet<[u8; 32]>,
    files_enabled: bool,
    raw_id: String,
}

impl RawDraft {
    pub(in crate::traces) fn new(
        project_id: &str,
        received: &ReceivedPayload,
        files_enabled: bool,
    ) -> Self {
        let encoded = if files_enabled {
            raw_payload::encode(&received.bytes, received.content)
        } else {
            EncodedRaw {
                record: raw_payload::wrap(&received.bytes, received.content),
                media: Vec::new(),
            }
        };
        Self {
            project_id: project_id.to_string(),
            raw_id: raw_id(project_id, received),
            received: received.clone(),
            encoded,
            inline: HashSet::new(),
            files_enabled,
        }
    }

    pub(in crate::traces) fn raw_id(&self) -> &str {
        &self.raw_id
    }

    pub(in crate::traces) fn project_id(&self) -> &str {
        &self.project_id
    }

    /// The media as pending file writes, each owned by the traces whose spans carry it.
    pub(in crate::traces) fn media_writes(
        &self,
        request: &ExportTraceServiceRequest,
    ) -> Vec<PendingFileWrite> {
        if self.encoded.media.is_empty() {
            return Vec::new();
        }
        let owners = media_owners(request);
        let every_trace: BTreeSet<String> = owners.values().flatten().cloned().collect();
        let mut writes = Vec::new();
        for media in &self.encoded.media {
            let text = base64_text(&media.bytes);
            let traces = owners.get(&media.hash).unwrap_or(&every_trace);
            for trace_id in traces {
                writes.push(PendingFileWrite {
                    project_id: self.project_id.clone(),
                    trace_id: trace_id.clone(),
                    hash: media.hash_hex(),
                    media_type: detect_mime_type_from_base64(text.as_bytes()).map(String::from),
                    size: media.bytes.len(),
                    data: text.clone().into_bytes(),
                    hash_algo: FILE_HASH_ALGORITHM.to_string(),
                });
            }
        }
        writes
    }

    /// Keep inline the media the file store refused for this project: `(project, uri)` as the file
    /// persistence reports it.
    pub(in crate::traces) fn keep_inline(&mut self, refused: &[(String, String)]) {
        let refused: HashSet<[u8; 32]> = refused
            .iter()
            .filter(|(project_id, _)| project_id == &self.project_id)
            .filter_map(|(_, uri)| parse_file_uri(uri))
            .filter_map(|parsed| hex::decode(parsed.hash).ok())
            .filter_map(|hash| <[u8; 32]>::try_from(hash).ok())
            .filter(|hash| self.encoded.media.iter().any(|media| &media.hash == hash))
            .collect();
        if refused.is_empty() {
            return;
        }
        self.inline.extend(refused);
        self.encoded = self.encode(&self.received.bytes, self.received.content);
    }

    fn encode(&self, bytes: &[u8], content: RawContent) -> EncodedRaw {
        if self.files_enabled {
            raw_payload::encode_with(bytes, content, |hash| !self.inline.contains(hash))
        } else {
            EncodedRaw {
                record: raw_payload::wrap(bytes, content),
                media: Vec::new(),
            }
        }
    }

    /// The version to store, given the identities the deletion fences kept.
    ///
    /// The fences that count are the ones after which the data must not exist anywhere: a deleted or
    /// tombstoned trace or session, a journalled deletion, a project being deleted, a timestamp no store can
    /// hold. When any of those removed a span, the version is the received export without it, re-encoded and
    /// marked [`RawOrigin::Fenced`]. Exact redeliveries are *not* removed here - their data is kept, under this
    /// record as well as the one that first carried it - so the caller passes the set from before that check.
    pub(in crate::traces) fn row(
        &self,
        request: &ExportTraceServiceRequest,
        keep: &HashSet<SpanKey>,
        at: DateTime<Utc>,
        hold_until: Option<DateTime<Utc>>,
    ) -> Result<RawRecordRow, String> {
        let everything = spans_of(request);
        let (origin, record) = if everything.iter().all(|key| keep.contains(key)) {
            (RawOrigin::Received, self.encoded.record.clone())
        } else {
            (RawOrigin::Fenced, self.filtered(keep)?)
        };
        Ok(self.version(
            request,
            keep,
            origin,
            record,
            at.timestamp_micros(),
            at,
            hold_until,
        ))
    }

    /// The version an ingest appends when, after its rows were written, the latest record does not hold them:
    /// the union of what the latest holds and what this ingest kept, two versions above the latest, so a
    /// reconciler's rewrite of an older read can never win over it (the `Check` step of the model).
    pub(in crate::traces) fn repair_row(
        &self,
        request: &ExportTraceServiceRequest,
        latest: Option<&RawRecordRow>,
        keep: &HashSet<SpanKey>,
        at: DateTime<Utc>,
        hold_until: Option<DateTime<Utc>>,
    ) -> Result<RawRecordRow, String> {
        let mut union = keep.clone();
        if let Some(latest) = latest {
            // An unreadable latest version holds nothing anyone can read back, so the repair supersedes it with
            // what this ingest carries rather than failing on it - failing would leave it in place for ever,
            // since every retry presents the same raw id and finds the same bytes.
            match record_identities(&latest.record) {
                Ok(held) => union.extend(held),
                Err(error) => tracing::error!(
                    raw_id = %latest.raw_id,
                    version = latest.version,
                    %error,
                    "The latest raw record is unreadable; superseding it with this ingest's version"
                ),
            }
        }
        let mut row = self.row(request, &union, at, hold_until)?;
        let floor = latest.map_or(0, |latest| latest.version.saturating_add(2));
        row.version = row.version.max(floor);
        Ok(row)
    }

    /// Whether `latest` holds every span of `written`, decoding it only when it could not.
    ///
    /// The received body holds every span it carried, so a version identical to this draft's own encoding
    /// needs no decoding. Anything else is decoded - a version another draft encoded with different media
    /// inline, a filtered one - and one that does not decode holds nothing, so the repair replaces it.
    pub(in crate::traces) fn covers(
        &self,
        latest: &RawRecordRow,
        written: &HashSet<SpanKey>,
    ) -> bool {
        if latest.origin == RawOrigin::Received && latest.record == self.encoded.record {
            return true;
        }
        record_identities(&latest.record)
            .is_ok_and(|held| written.iter().all(|key| held.contains(key)))
    }

    /// The received export without the spans outside `keep`, as protobuf, media cut out as in the first version.
    fn filtered(&self, keep: &HashSet<SpanKey>) -> Result<Vec<u8>, String> {
        let original = request_of(self.received.content, &self.received.bytes)?;
        let bytes = without_spans(original, keep).encode_to_vec();
        Ok(self.encode(&bytes, RawContent::Protobuf).record)
    }

    #[allow(clippy::too_many_arguments)]
    fn version(
        &self,
        request: &ExportTraceServiceRequest,
        keep: &HashSet<SpanKey>,
        origin: RawOrigin,
        record: Vec<u8>,
        version: i64,
        at: DateTime<Utc>,
        hold_until: Option<DateTime<Utc>>,
    ) -> RawRecordRow {
        let (signal_until, trace_ids) = extent(request, keep);
        RawRecordRow {
            project_id: ProjectId::from(self.project_id.as_str()),
            raw_id: self.raw_id.clone(),
            signal: StagedSignal::Traces,
            received_at: at,
            origin,
            version,
            signal_until: signal_until.unwrap_or(at),
            hold_until,
            trace_ids,
            record,
        }
    }
}

/// Hex BLAKE3 of the project and the received body: the same body for the same project is the same record,
/// whatever its media's fate.
fn raw_id(project_id: &str, received: &ReceivedPayload) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"sideseat-raw-record-v1\0");
    hasher.update(project_id.as_bytes());
    hasher.update(&[0]);
    hasher.update(&[u8::from(received.content == RawContent::Json)]);
    hasher.update(&received.bytes);
    hasher.finalize().to_hex().to_string()
}

fn base64_text(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A received body as the request it encodes, before any mutation.
pub(in crate::traces) fn request_of(
    content: RawContent,
    bytes: &[u8],
) -> Result<ExportTraceServiceRequest, String> {
    match content {
        RawContent::Protobuf => ExportTraceServiceRequest::decode(bytes)
            .map_err(|error| format!("raw protobuf is invalid: {error}")),
        RawContent::Json => serde_json::from_slice(bytes)
            .map_err(|error| format!("raw OTLP/JSON is invalid: {error}")),
    }
}

/// The span identities a stored record holds, read from its shape: no media needed.
pub(in crate::traces) fn record_identities(record: &[u8]) -> Result<HashSet<SpanKey>, String> {
    crate::raw_identities::record_identities(record)
}

/// Every span identity of a request, in order.
pub(in crate::traces) fn spans_of(request: &ExportTraceServiceRequest) -> Vec<SpanKey> {
    request
        .resource_spans
        .iter()
        .flat_map(|rs| &rs.scope_spans)
        .flat_map(|ss| &ss.spans)
        .map(|span| (hex::encode(&span.trace_id), hex::encode(&span.span_id)))
        .collect()
}

/// The request without the spans outside `keep`, empty scopes and resources dropped.
pub(in crate::traces) fn without_spans(
    mut request: ExportTraceServiceRequest,
    keep: &HashSet<SpanKey>,
) -> ExportTraceServiceRequest {
    for resource_spans in &mut request.resource_spans {
        for scope_spans in &mut resource_spans.scope_spans {
            scope_spans.spans.retain(|span| {
                keep.contains(&(hex::encode(&span.trace_id), hex::encode(&span.span_id)))
            });
        }
        resource_spans
            .scope_spans
            .retain(|scope_spans| !scope_spans.spans.is_empty());
    }
    request
        .resource_spans
        .retain(|resource_spans| !resource_spans.scope_spans.is_empty());
    request
}

/// The latest span start among the kept spans, and their distinct traces.
fn extent(
    request: &ExportTraceServiceRequest,
    keep: &HashSet<SpanKey>,
) -> (Option<DateTime<Utc>>, Vec<String>) {
    let mut latest: Option<u64> = None;
    let mut traces = BTreeSet::new();
    for span in request
        .resource_spans
        .iter()
        .flat_map(|rs| &rs.scope_spans)
        .flat_map(|ss| &ss.spans)
    {
        let key = (hex::encode(&span.trace_id), hex::encode(&span.span_id));
        if keep.contains(&key) {
            latest = latest.max(Some(span.start_time_unix_nano));
            traces.insert(key.0);
        }
    }
    let latest = latest
        .and_then(|nanos| i64::try_from(nanos).ok())
        .map(DateTime::from_timestamp_nanos);
    (latest, traces.into_iter().collect())
}

/// Which traces' spans carry each media object, by scanning every string a span holds.
fn media_owners(request: &ExportTraceServiceRequest) -> BTreeMap<[u8; 32], BTreeSet<String>> {
    let mut owners: BTreeMap<[u8; 32], BTreeSet<String>> = BTreeMap::new();
    for resource_spans in &request.resource_spans {
        for scope_spans in &resource_spans.scope_spans {
            for span in &scope_spans.spans {
                let trace_id = hex::encode(&span.trace_id);
                let mut hashes = Vec::new();
                collect_attributes(&span.attributes, &mut hashes);
                for event in &span.events {
                    collect_attributes(&event.attributes, &mut hashes);
                }
                for link in &span.links {
                    collect_attributes(&link.attributes, &mut hashes);
                }
                for hash in hashes {
                    owners.entry(hash).or_default().insert(trace_id.clone());
                }
            }
        }
    }
    owners
}

fn collect_attributes(attributes: &[KeyValue], out: &mut Vec<[u8; 32]>) {
    for attribute in attributes {
        if let Some(value) = &attribute.value {
            collect_value(value, out);
        }
    }
}

fn collect_value(value: &AnyValue, out: &mut Vec<[u8; 32]>) {
    match &value.value {
        Some(any_value::Value::StringValue(text)) if text.len() >= raw_payload::MIN_MEDIA_RUN => {
            out.extend(
                raw_payload::media_in(text.as_bytes())
                    .into_iter()
                    .map(|m| m.hash),
            );
        }
        Some(any_value::Value::ArrayValue(array)) => {
            for item in &array.values {
                collect_value(item, out);
            }
        }
        Some(any_value::Value::KvlistValue(list)) => collect_attributes(&list.values, out),
        _ => {}
    }
}

#[cfg(test)]
#[path = "raw_tests.rs"]
mod tests;
