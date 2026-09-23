//! Stable identity for OTLP log records.

use opentelemetry_proto::tonic::common::v1::InstrumentationScope;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use opentelemetry_proto::tonic::resource::v1::Resource;

use crate::otlp::PROJECT_ID_ATTR;

/// Digest every producer-supplied field that can distinguish one log record from another.
pub fn log_digest(
    record: &LogRecord,
    resource: Option<&Resource>,
    resource_schema_url: &str,
    scope: Option<&InstrumentationScope>,
    scope_schema_url: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&record.time_unix_nano.to_le_bytes());
    hasher.update(&record.observed_time_unix_nano.to_le_bytes());
    hasher.update(&record.severity_number.to_le_bytes());
    write_str(&mut hasher, &record.severity_text);
    write_any_value(&mut hasher, record.body.as_ref());
    write_attributes(&mut hasher, &record.attributes, false);
    hasher.update(&record.dropped_attributes_count.to_le_bytes());
    hasher.update(&record.flags.to_le_bytes());
    write_bytes(&mut hasher, &record.trace_id);
    write_bytes(&mut hasher, &record.span_id);
    write_str(&mut hasher, &record.event_name);

    match resource {
        Some(resource) => {
            hasher.update(&[1]);
            write_attributes(&mut hasher, &resource.attributes, true);
            hasher.update(&resource.dropped_attributes_count.to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    };
    write_str(&mut hasher, resource_schema_url);

    match scope {
        Some(scope) => {
            hasher.update(&[1]);
            write_str(&mut hasher, &scope.name);
            write_str(&mut hasher, &scope.version);
            write_attributes(&mut hasher, &scope.attributes, false);
            hasher.update(&scope.dropped_attributes_count.to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    };
    write_str(&mut hasher, scope_schema_url);

    hasher.finalize().to_hex().to_string()
}

fn write_attributes(hasher: &mut blake3::Hasher, attributes: &[KeyValue], skip_project: bool) {
    let mut values: Vec<&KeyValue> = attributes
        .iter()
        .filter(|value| !(skip_project && value.key == PROJECT_ID_ATTR))
        .collect();
    values.sort_by(|left, right| left.key.cmp(&right.key));
    hasher.update(&(values.len() as u64).to_le_bytes());
    for value in values {
        write_str(hasher, &value.key);
        write_any_value(hasher, value.value.as_ref());
    }
}

fn write_any_value(hasher: &mut blake3::Hasher, value: Option<&AnyValue>) {
    let Some(value) = value.and_then(|value| value.value.as_ref()) else {
        hasher.update(&[0]);
        return;
    };
    match value {
        any_value::Value::StringValue(value) => {
            hasher.update(&[1]);
            write_str(hasher, value);
        }
        any_value::Value::BoolValue(value) => {
            hasher.update(&[2, u8::from(*value)]);
        }
        any_value::Value::IntValue(value) => {
            hasher.update(&[3]);
            hasher.update(&value.to_le_bytes());
        }
        any_value::Value::DoubleValue(value) => {
            hasher.update(&[4]);
            hasher.update(&value.to_bits().to_le_bytes());
        }
        any_value::Value::BytesValue(value) => {
            hasher.update(&[5]);
            write_bytes(hasher, value);
        }
        any_value::Value::ArrayValue(value) => {
            hasher.update(&[6]);
            hasher.update(&(value.values.len() as u64).to_le_bytes());
            for item in &value.values {
                write_any_value(hasher, Some(item));
            }
        }
        any_value::Value::KvlistValue(value) => {
            hasher.update(&[7]);
            write_attributes(hasher, &value.values, false);
        }
    }
}

fn write_str(hasher: &mut blake3::Hasher, value: &str) {
    write_bytes(hasher, value.as_bytes());
}

fn write_bytes(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};

    fn text(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(AnyValue {
                value: Some(any_value::Value::StringValue(value.to_string())),
            }),
        }
    }

    #[test]
    fn resource_scope_and_schema_are_identity() {
        let record = LogRecord {
            time_unix_nano: 7,
            body: Some(AnyValue {
                value: Some(any_value::Value::StringValue("same".to_string())),
            }),
            ..Default::default()
        };
        let resource_a = Resource {
            attributes: vec![text("service.name", "a")],
            ..Default::default()
        };
        let resource_b = Resource {
            attributes: vec![text("service.name", "b")],
            ..Default::default()
        };
        let a = log_digest(&record, Some(&resource_a), "resource/v1", None, "scope/v1");
        let b = log_digest(&record, Some(&resource_b), "resource/v1", None, "scope/v1");
        let schema = log_digest(&record, Some(&resource_a), "resource/v2", None, "scope/v1");
        assert_ne!(a, b);
        assert_ne!(a, schema);
    }

    #[test]
    fn injected_project_attribute_is_not_producer_content() {
        let record = LogRecord::default();
        let one = Resource {
            attributes: vec![text(PROJECT_ID_ATTR, "one"), text("service.name", "api")],
            ..Default::default()
        };
        let two = Resource {
            attributes: vec![text(PROJECT_ID_ATTR, "two"), text("service.name", "api")],
            ..Default::default()
        };
        assert_eq!(
            log_digest(&record, Some(&one), "", None, ""),
            log_digest(&record, Some(&two), "", None, "")
        );
    }
}
