//! A decoded log record's content (`domain::log_record`) read from the record itself, so ingest derives its
//! messages by the same function a read derives them by from the stored raw record.

use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use sideseat_domain::log_record::{LogRecordContent, OtlpValue};

/// The record's content, its values as they stand.
pub fn log_record_content(record: &LogRecord) -> LogRecordContent {
    // All-zero ids are OpenTelemetry's "invalid".
    let valid = |id: &[u8]| !id.is_empty() && id.iter().any(|byte| *byte != 0);
    LogRecordContent {
        names_a_span: valid(&record.trace_id) && valid(&record.span_id),
        time_unix_nano: record.time_unix_nano,
        observed_time_unix_nano: record.observed_time_unix_nano,
        event_name: record.event_name.clone(),
        attributes: pairs(&record.attributes),
        body: record.body.as_ref().map(value),
    }
}

fn pairs(pairs: &[KeyValue]) -> Vec<(String, Option<OtlpValue>)> {
    pairs
        .iter()
        .map(|pair| (pair.key.clone(), pair.value.as_ref().map(value)))
        .collect()
}

fn value(value: &AnyValue) -> OtlpValue {
    match &value.value {
        None => OtlpValue::Empty,
        Some(any_value::Value::StringValue(text)) => OtlpValue::String(text.clone()),
        Some(any_value::Value::BoolValue(flag)) => OtlpValue::Bool(*flag),
        Some(any_value::Value::IntValue(number)) => OtlpValue::Int(*number),
        Some(any_value::Value::DoubleValue(number)) => OtlpValue::Double(*number),
        Some(any_value::Value::BytesValue(bytes)) => OtlpValue::Bytes(bytes.clone()),
        Some(any_value::Value::ArrayValue(items)) => {
            OtlpValue::Array(items.values.iter().map(self::value).collect())
        }
        Some(any_value::Value::KvlistValue(members)) => OtlpValue::Kvlist(pairs(&members.values)),
    }
}
