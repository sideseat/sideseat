//! Flatten OTLP resource/scope logs into storage rows.

use std::collections::HashMap;

use crate::otlp::{
    PROJECT_ID_ATTR, any_value_to_json, any_value_to_string, attrs_to_typed_json,
    extract_attributes, get_environment, get_session_id, get_user_id, keys,
};
use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::AnyValue;
use sideseat_core::utils::time::nanos_to_datetime;
use sideseat_ports::types::NormalizedLog;

use super::identity::log_digest;

pub fn extract_logs_batch(
    request: &ExportLogsServiceRequest,
    received_at: DateTime<Utc>,
) -> Vec<NormalizedLog> {
    let mut result = Vec::new();
    let mut ordinals: HashMap<String, u32> = HashMap::new();

    for resource_logs in &request.resource_logs {
        let resource = resource_logs.resource.as_ref();
        let resource_map = resource
            .map(|resource| extract_attributes(&resource.attributes))
            .unwrap_or_default();
        let resource_attributes = resource
            .map(|resource| attrs_to_typed_json(&resource.attributes))
            .unwrap_or_default();

        for scope_logs in &resource_logs.scope_logs {
            let scope = scope_logs.scope.as_ref();
            let scope_attributes = scope
                .map(|scope| attrs_to_typed_json(&scope.attributes))
                .unwrap_or_default();
            for record in &scope_logs.log_records {
                let digest = log_digest(
                    record,
                    resource,
                    &resource_logs.schema_url,
                    scope,
                    &scope_logs.schema_url,
                );
                let ordinal = ordinals.entry(digest.clone()).or_default();
                let this_ordinal = *ordinal;
                *ordinal = ordinal.saturating_add(1);

                let time =
                    (record.time_unix_nano != 0).then(|| nanos_to_datetime(record.time_unix_nano));
                let observed_time = (record.observed_time_unix_nano != 0)
                    .then(|| nanos_to_datetime(record.observed_time_unix_nano));
                let attributes_map = extract_attributes(&record.attributes);
                let body = record
                    .body
                    .as_ref()
                    .map(any_value_to_json)
                    .unwrap_or_default();
                let body_text = record.body.as_ref().and_then(body_text);

                let mut log = NormalizedLog {
                    project_id: resource_map.get(PROJECT_ID_ATTR).cloned(),
                    log_digest: digest,
                    ordinal: this_ordinal,
                    timestamp: time.or(observed_time).unwrap_or(received_at),
                    time,
                    observed_time,
                    severity_number: record.severity_number,
                    severity_text: nonempty(&record.severity_text),
                    body,
                    body_text,
                    attributes: attrs_to_typed_json(&record.attributes),
                    dropped_attributes_count: record.dropped_attributes_count,
                    flags: record.flags,
                    trace_id: nonempty_hex(&record.trace_id),
                    span_id: nonempty_hex(&record.span_id),
                    event_name: nonempty(&record.event_name),
                    session_id: get_session_id(&attributes_map)
                        .or_else(|| get_session_id(&resource_map)),
                    user_id: get_user_id(&attributes_map).or_else(|| get_user_id(&resource_map)),
                    environment: get_environment(&attributes_map)
                        .or_else(|| get_environment(&resource_map)),
                    service_name: resource_map.get(keys::SERVICE_NAME).cloned(),
                    service_version: resource_map.get(keys::SERVICE_VERSION).cloned(),
                    service_namespace: resource_map.get(keys::SERVICE_NAMESPACE).cloned(),
                    service_instance_id: resource_map.get(keys::SERVICE_INSTANCE_ID).cloned(),
                    resource_attributes: resource_attributes.clone(),
                    scope_name: scope.and_then(|scope| nonempty(&scope.name)),
                    scope_version: scope.and_then(|scope| nonempty(&scope.version)),
                    scope_attributes: scope_attributes.clone(),
                    scope_schema_url: nonempty(&scope_logs.schema_url),
                    resource_schema_url: nonempty(&resource_logs.schema_url),
                    raw_log: serde_json::to_value(record).unwrap_or_default(),
                    ingested_at: Some(received_at),
                    hold_until: None,
                    logical_bytes: 0,
                    search: Default::default(),
                };
                log.logical_bytes = crate::domain::accounting::log_logical_bytes(&log);
                result.push(log);
            }
        }
    }
    result
}

fn body_text(value: &AnyValue) -> Option<String> {
    let rendered = any_value_to_string(value);
    (!rendered.is_empty()).then_some(rendered)
}

fn nonempty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn nonempty_hex(value: &[u8]) -> Option<String> {
    (!value.is_empty()).then(|| hex::encode(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};

    #[test]
    fn repeated_records_get_stable_ordinals_and_retry_identical_ids() {
        let record = LogRecord {
            time_unix_nano: 1_700_000_000_000_000_000,
            body: Some(AnyValue {
                value: Some(any_value::Value::StringValue("repeat".to_string())),
            }),
            ..Default::default()
        };
        let request = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                scope_logs: vec![ScopeLogs {
                    log_records: vec![record.clone(), record],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let now = DateTime::from_timestamp(1_700_000_100, 0).unwrap();
        let first = extract_logs_batch(&request, now);
        let retry = extract_logs_batch(&request, now + chrono::TimeDelta::seconds(1));
        assert_eq!(first[0].log_digest, first[1].log_digest);
        assert_eq!((first[0].ordinal, first[1].ordinal), (0, 1));
        assert_eq!(
            first
                .iter()
                .map(|row| (&row.log_digest, row.ordinal))
                .collect::<Vec<_>>(),
            retry
                .iter()
                .map(|row| (&row.log_digest, row.ordinal))
                .collect::<Vec<_>>()
        );
    }
}
