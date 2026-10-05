//! Messages carried by log records that name a span.
//!
//! Some instrumentations report the conversation as log records linked to a span rather than as span
//! events. The record is stored as a log either way; a record a declared `log_events` entry recognises also
//! has its messages read here, by the reader span events use, and stored beside it. They are joined to the
//! span at read time and never written into the span's row: span rows are latest-ingest-wins and may arrive
//! after the log, and the log is durable before acknowledgement while the span may still be queued.

use opentelemetry_proto::tonic::common::v1::any_value;
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use sideseat_core::utils::time::nanos_to_datetime;
use sideseat_domain::observations::RawMessage;
use sideseat_domain::rules::schema::LogEventPayload;

use crate::message_events::{EventSpan, read_message_event, structured_attributes};
use crate::otlp::extract_attributes;

/// The raw messages one log record carries, or none.
///
/// Nothing is read from a record that names no span, since nothing could attach it to a conversation, nor
/// from one no declaration recognises. The instant is the record's own time, falling back to its observed
/// time - never the receipt time, so a re-delivered record reads exactly as it did the first time.
pub(super) fn log_record_messages(record: &LogRecord) -> Vec<RawMessage> {
    if !names_a_span(record) {
        return Vec::new();
    }
    let attributes = extract_attributes(&record.attributes);
    let event_name = (!record.event_name.is_empty()).then_some(record.event_name.as_str());
    let Some(declared) = sideseat_domain::rules::ruleset()
        .log_events
        .recognise(event_name, |key| attributes.get(key).map(String::as_str))
    else {
        return Vec::new();
    };
    let payload = match declared.payload {
        LogEventPayload::BodyMembers => {
            match record.body.as_ref().and_then(|body| body.value.as_ref()) {
                Some(any_value::Value::KvlistValue(members)) => {
                    structured_attributes(&members.values)
                }
                // A body that is not a map has no members, which reads as an event with no attributes - what
                // the same event emitted on a span with none would read as.
                _ => Default::default(),
            }
        }
        LogEventPayload::Attributes => structured_attributes(&record.attributes),
    };
    let nanos = if record.time_unix_nano != 0 {
        record.time_unix_nano
    } else {
        record.observed_time_unix_nano
    };
    read_message_event(
        &declared.name,
        &payload,
        nanos_to_datetime(nanos),
        EventSpan::unattached(),
    )
}

/// Whether the record carries a usable trace and span id. All-zero ids are OpenTelemetry's "invalid".
fn names_a_span(record: &LogRecord) -> bool {
    let valid = |id: &[u8]| !id.is_empty() && id.iter().any(|byte| *byte != 0);
    valid(&record.trace_id) && valid(&record.span_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, ArrayValue, KeyValue, KeyValueList};
    use serde_json::json;
    use sideseat_domain::observations::MessageSource;

    fn text(value: &str) -> AnyValue {
        AnyValue {
            value: Some(any_value::Value::StringValue(value.to_string())),
        }
    }

    fn kv(key: &str, value: AnyValue) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(value),
        }
    }

    fn map(members: Vec<KeyValue>) -> AnyValue {
        AnyValue {
            value: Some(any_value::Value::KvlistValue(KeyValueList {
                values: members,
            })),
        }
    }

    fn linked(record: LogRecord) -> LogRecord {
        LogRecord {
            time_unix_nano: 1_700_000_000_000_000_000,
            trace_id: vec![1; 16],
            span_id: vec![2; 8],
            ..record
        }
    }

    fn sources(messages: &[RawMessage]) -> Vec<String> {
        messages
            .iter()
            .map(|message| match &message.source {
                MessageSource::Event { name, .. } => format!("event:{name}"),
                MessageSource::Attribute { key, .. } => format!("attr:{key}"),
            })
            .collect()
    }

    #[test]
    fn body_members_are_the_event_attributes() {
        let record = linked(LogRecord {
            event_name: "gen_ai.choice".to_string(),
            body: Some(map(vec![
                kv(
                    "index",
                    AnyValue {
                        value: Some(any_value::Value::IntValue(0)),
                    },
                ),
                kv("finish_reason", text("stop")),
                kv(
                    "message",
                    map(vec![
                        kv("role", text("assistant")),
                        kv(
                            "content",
                            AnyValue {
                                value: Some(any_value::Value::ArrayValue(ArrayValue {
                                    values: vec![map(vec![kv("text", text("Paris"))])],
                                })),
                            },
                        ),
                    ]),
                ),
            ])),
            ..Default::default()
        });
        let messages = log_record_messages(&record);
        assert_eq!(sources(&messages), vec!["event:gen_ai.choice".to_string()]);
        assert_eq!(
            messages[0].content["message"],
            json!({"role": "assistant", "content": [{"text": "Paris"}]}),
            "a nested body member keeps its structure"
        );
        assert_eq!(messages[0].content["finish_reason"], json!("stop"));
        let MessageSource::Event { time, .. } = &messages[0].source else {
            unreachable!()
        };
        assert_eq!(time.timestamp(), 1_700_000_000, "the record's own instant");
    }

    #[test]
    fn the_legacy_event_name_attribute_names_the_event() {
        let record = linked(LogRecord {
            attributes: vec![kv("event.name", text("gen_ai.user.message"))],
            body: Some(map(vec![kv(
                "content",
                text("What is the capital of France?"),
            )])),
            ..Default::default()
        });
        let messages = log_record_messages(&record);
        assert_eq!(
            sources(&messages),
            vec!["event:gen_ai.user.message".to_string()]
        );
        assert_eq!(
            messages[0].content,
            json!({"content": "What is the capital of France?"}),
            "body members only: the naming attribute is not one of the event's"
        );
    }

    #[test]
    fn an_attributes_payload_reads_the_container_readings() {
        let input = json!([{"role": "user", "parts": [{"type": "text", "content": "hi"}]}]);
        let output =
            json!([{"role": "assistant", "parts": [{"type": "text", "content": "hello"}]}]);
        let record = linked(LogRecord {
            event_name: "gen_ai.client.inference.operation.details".to_string(),
            attributes: vec![
                kv("gen_ai.input.messages", text(&input.to_string())),
                kv("gen_ai.output.messages", text(&output.to_string())),
            ],
            ..Default::default()
        });
        let messages = log_record_messages(&record);
        assert!(
            !messages.is_empty()
                && !sources(&messages)
                    .contains(&"event:gen_ai.client.inference.operation.details".to_string()),
            "the container is replaced by its readings, as on a span: {:?}",
            sources(&messages)
        );
        let rendered = serde_json::to_string(&messages).unwrap();
        assert!(
            rendered.contains("hello") && rendered.contains("hi"),
            "{rendered}"
        );
    }

    #[test]
    fn nothing_is_read_without_a_span_or_a_declaration() {
        let body = Some(map(vec![kv("content", text("hi"))]));
        let unlinked = LogRecord {
            event_name: "gen_ai.user.message".to_string(),
            body: body.clone(),
            ..Default::default()
        };
        assert!(log_record_messages(&unlinked).is_empty(), "no span ids");
        let zero = LogRecord {
            trace_id: vec![0; 16],
            span_id: vec![0; 8],
            ..unlinked.clone()
        };
        assert!(log_record_messages(&zero).is_empty(), "invalid span ids");
        let undeclared = linked(LogRecord {
            event_name: "app.checkout".to_string(),
            body,
            ..Default::default()
        });
        assert!(
            log_record_messages(&undeclared).is_empty(),
            "no declaration"
        );
    }
}
