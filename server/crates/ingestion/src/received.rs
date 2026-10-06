//! The request body exactly as it was received, and the staged form that carries it until it is stored.
//!
//! Staging used to hold `request.encode_to_vec()` after the project attribute had been injected and unstorable
//! records stripped - a re-encoding of a mutated message, so the bytes the producer sent existed nowhere once
//! the request returned. Staging now holds the received body itself, wrapped as a media-free raw record
//! ([`raw_payload::wrap`]). Staging is a bounded durability buffer: the payload is released once its records
//! are confirmed stored, as before.
//!
//! The mutations are deterministic, so the reader of a staged payload re-applies them: decode, inject the
//! staging row's project, strip what cannot be stored. A payload staged by an earlier version is a bare
//! protobuf of the already-mutated request, which the same steps leave unchanged.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use serde::de::DeserializeOwned;
use sideseat_domain::raw_payload::{self, RawContent};

use crate::otlp::{inject_project_id_logs, inject_project_id_metrics, inject_project_id_traces};

/// A request body as it arrived on the wire, after content-encoding was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedPayload {
    pub bytes: Vec<u8>,
    pub content: RawContent,
}

impl ReceivedPayload {
    pub fn new(bytes: impl Into<Vec<u8>>, content: RawContent) -> Self {
        Self {
            bytes: bytes.into(),
            content,
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
    if !raw_payload::is_record(bytes) {
        return T::decode(bytes).map_err(|error| format!("staged protobuf is invalid: {error}"));
    }
    let (content, raw) = raw_payload::decode(bytes, |_| None).map_err(|error| error.to_string())?;
    match content {
        RawContent::Protobuf => T::decode(raw.as_slice())
            .map_err(|error| format!("staged protobuf is invalid: {error}")),
        RawContent::Json => serde_json::from_slice(&raw)
            .map_err(|error| format!("staged OTLP/JSON is invalid: {error}")),
    }
}

/// A staged trace export, prepared exactly as the request path prepared it.
pub fn staged_traces(bytes: &[u8], project_id: &str) -> Result<ExportTraceServiceRequest, String> {
    let mut request = decode_staged::<ExportTraceServiceRequest>(bytes)?;
    inject_project_id_traces(&mut request, project_id);
    crate::traces::strip_unstorable_spans(&mut request);
    Ok(request)
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
        let received = ReceivedPayload::new(json.clone(), RawContent::Json);
        let staged = received.staged();
        assert_eq!(
            raw_payload::decode(&staged, |_| None).unwrap(),
            (RawContent::Json, json)
        );
        assert_eq!(
            staged_traces(&staged, "project").unwrap(),
            prepared("project")
        );

        let protobuf = ReceivedPayload::new(export().encode_to_vec(), RawContent::Protobuf);
        assert_eq!(
            staged_traces(&protobuf.staged(), "project").unwrap(),
            prepared("project")
        );
    }

    /// A payload staged before this version holds the already-prepared request; preparing it again is a no-op.
    #[test]
    fn a_payload_staged_by_an_earlier_version_still_decodes() {
        let legacy = prepared("project").encode_to_vec();
        assert_eq!(
            staged_traces(&legacy, "project").unwrap(),
            prepared("project")
        );
    }
}
