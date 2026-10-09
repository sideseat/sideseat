//! The request body exactly as it was received, and the staged form that carries it until it is stored.
//!
//! Staging used to hold `request.encode_to_vec()` after the project attribute had been injected and unstorable
//! records stripped - a re-encoding of a mutated message, so the bytes the producer sent existed nowhere once
//! the request returned. Staging now holds the received body itself, wrapped as a media-free raw record
//! ([`raw_payload::wrap`]). Staging is a bounded durability buffer: the payload is released once its records
//! are confirmed stored, as before.
//!
//! The mutations are deterministic, so the reader of a staged payload re-applies them: decode, inject the
//! staging row's project, strip what cannot be stored.

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use serde::de::DeserializeOwned;
use sideseat_domain::raw_payload::{self, RawContent};

use crate::otlp::{inject_project_id_logs, inject_project_id_metrics, inject_project_id_traces};

/// A request body as it arrived on the wire, after content-encoding was removed, and when it arrived.
///
/// The receipt orders an export's revisions of a span: every write of the export - the requester's, a consumer's,
/// redrive's, however late - stores its rows at this instant, so a copy written after a revision received later
/// is superseded at once instead of taking over from it. Staging stores it with the payload, and a payload read
/// back carries it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedPayload {
    pub bytes: Vec<u8>,
    pub content: RawContent,
    pub received_at: DateTime<Utc>,
}

impl ReceivedPayload {
    /// The receipt is kept to the microsecond, the precision every store holds it at, so an instant read back
    /// compares equal to the one written.
    pub fn new(bytes: impl Into<Vec<u8>>, content: RawContent, received_at: DateTime<Utc>) -> Self {
        Self {
            bytes: bytes.into(),
            content,
            received_at: DateTime::from_timestamp_micros(received_at.timestamp_micros())
                .unwrap_or(received_at),
        }
    }

    /// What staging stores for this payload.
    pub fn staged(&self) -> Vec<u8> {
        raw_payload::wrap(&self.bytes, self.content)
    }
}

/// Decode staged bytes into the request they carry, before any mutation.
pub fn decode_staged<T>(bytes: &[u8]) -> Result<T, String>
where
    T: Message + Default + DeserializeOwned,
{
    let (content, raw) = raw_payload::decode(bytes, |_| None).map_err(|error| error.to_string())?;
    match content {
        RawContent::Protobuf => T::decode(raw.as_slice())
            .map_err(|error| format!("staged protobuf is invalid: {error}")),
        RawContent::Json => serde_json::from_slice(&raw)
            .map_err(|error| format!("staged OTLP/JSON is invalid: {error}")),
    }
}

/// The body a staged payload carries, as it was received at `received_at`.
pub fn staged_received(
    bytes: &[u8],
    received_at: DateTime<Utc>,
) -> Result<ReceivedPayload, String> {
    let (content, raw) = raw_payload::decode(bytes, |_| None).map_err(|error| error.to_string())?;
    Ok(ReceivedPayload::new(raw, content, received_at))
}

/// A staged trace export, prepared exactly as the request path prepared it, with the body it was received as at
/// `received_at` - the staged payload's `created_at`.
pub fn staged_traces(
    bytes: &[u8],
    project_id: &str,
    received_at: DateTime<Utc>,
) -> Result<(ExportTraceServiceRequest, ReceivedPayload), String> {
    let mut request = decode_staged::<ExportTraceServiceRequest>(bytes)?;
    inject_project_id_traces(&mut request, project_id);
    crate::traces::strip_unstorable_spans(&mut request);
    Ok((request, staged_received(bytes, received_at)?))
}

/// A staged metric export, prepared exactly as the request path prepared it.
pub fn staged_metrics(
    bytes: &[u8],
    project_id: &str,
) -> Result<ExportMetricsServiceRequest, String> {
    let mut request = decode_staged::<ExportMetricsServiceRequest>(bytes)?;
    inject_project_id_metrics(&mut request, project_id);
    crate::signals::strip_unstorable_metrics(&mut request);
    Ok(request)
}

/// A staged log export, prepared exactly as the request path prepared it.
pub fn staged_logs(bytes: &[u8], project_id: &str) -> Result<ExportLogsServiceRequest, String> {
    let mut request = decode_staged::<ExportLogsServiceRequest>(bytes)?;
    inject_project_id_logs(&mut request, project_id);
    crate::signals::strip_unstorable_logs(&mut request);
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

    fn export() -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource::default()),
                scope_spans: vec![ScopeSpans {
                    spans: vec![
                        Span {
                            trace_id: vec![1; 16],
                            span_id: vec![2; 8],
                            name: "kept".into(),
                            start_time_unix_nano: 1_700_000_000_000_000_000,
                            end_time_unix_nano: 1_700_000_001_000_000_000,
                            ..Default::default()
                        },
                        // No start time: not storable, stripped on both paths.
                        Span {
                            trace_id: vec![1; 16],
                            span_id: vec![3; 8],
                            name: "stripped".into(),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        }
    }

    /// What the request path hands to persistence.
    fn prepared(project_id: &str) -> ExportTraceServiceRequest {
        let mut request = export();
        inject_project_id_traces(&mut request, project_id);
        crate::traces::strip_unstorable_spans(&mut request);
        request
    }

    #[test]
    fn a_staged_body_is_the_received_bytes_and_prepares_like_the_request_path() {
        let json = serde_json::to_vec(&export()).unwrap();
        let at = Utc::now();
        let received = ReceivedPayload::new(json.clone(), RawContent::Json, at);
        let staged = received.staged();
        assert_eq!(
            raw_payload::decode(&staged, |_| None).unwrap(),
            (RawContent::Json, json)
        );
        assert_eq!(
            staged_traces(&staged, "project", at).unwrap(),
            (prepared("project"), received)
        );

        let protobuf = ReceivedPayload::new(export().encode_to_vec(), RawContent::Protobuf, at);
        assert_eq!(
            staged_traces(&protobuf.staged(), "project", at).unwrap().0,
            prepared("project")
        );
    }

    /// A receipt read back from a store, which keeps microseconds, is the receipt the export was written with.
    #[test]
    fn a_receipt_is_kept_to_the_microsecond() {
        let at = DateTime::from_timestamp(1_700_000_000, 123_456_789).unwrap();
        let received = ReceivedPayload::new(Vec::new(), RawContent::Protobuf, at);
        assert_eq!(received.received_at.timestamp_subsec_nanos(), 123_456_000);
    }
}
