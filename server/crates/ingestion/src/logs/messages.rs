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
pub(super) fn log_record_messages(record: &LogRecord, scope: Option<&str>) -> Vec<RawMessage> {
    if !names_a_span(record) {
        return Vec::new();
    }
    let Some((name, payload)) = log_event_payload(record) else {
        return Vec::new();
    };
    let nanos = if record.time_unix_nano != 0 {
        record.time_unix_nano
    } else {
        record.observed_time_unix_nano
    };
    read_message_event(
        &name,
        &payload,
        nanos_to_datetime(nanos),
        EventSpan::unattached(scope),
    )
}

/// The key the frame this record carries states, where its messages were read from a carrier that frames requests
/// (`frames_requests`): the attribute the declaration names, read from the payload the frame was read from -
/// the record's attributes or its body's members - through the renderer a request's key is read through
/// (`crate::frame_keys`). `None` where no message is such a frame, and where two of them state different keys,
/// which names no one request.
pub(super) fn log_record_frame_key(record: &LogRecord, messages: &[RawMessage]) -> Option<String> {
    let frames = sideseat_domain::rules::ruleset().carriers.frames();
    if frames.is_empty() {
        return None;
    }
    // The carriers are asked first, so a record whose messages frame nothing is not read again.
    let attributes: Vec<&str> = messages
        .iter()
        .filter_map(|message| match &message.source {
            sideseat_domain::observations::MessageSource::Event { name, .. } => {
                frames.frame_attribute(name)
            }
            sideseat_domain::observations::MessageSource::Attribute { .. } => None,
        })
        .collect();
    if attributes.is_empty() {
        return None;
    }
    let (_, payload, _) = recognised(record)?;
    let mut keys = attributes.iter().filter_map(|attribute| match payload {
        LogEventPayload::Attributes => {
            crate::frame_keys::attribute_key(&record.attributes, attribute)
        }
        LogEventPayload::BodyMembers => body_member_key(record, attribute),
    });
    let first = keys.next()?;
    keys.all(|key| key == first).then_some(first)
}

/// The key a member of the record's body states, read as `body_members` reads the body.
fn body_member_key(record: &LogRecord, member: &str) -> Option<String> {
    match record.body.as_ref().and_then(|body| body.value.as_ref())? {
        any_value::Value::KvlistValue(members) => {
            crate::frame_keys::attribute_key(&members.values, member)
        }
        any_value::Value::StringValue(text) => match serde_json::from_str(text) {
            Ok(serde_json::Value::Object(members)) => members
                .get(member)
                .and_then(crate::frame_keys::json_key_text),
            _ => None,
        },
        _ => None,
    }
}

/// The message event a log record is declared to carry: its name, and the attributes the event reader
/// takes - the record's body members or its own attributes, as the `log_events` declaration says.
///
/// `None` when no declaration recognises the record. Public so a corpus measurement can ask the event plan
/// exactly what ingestion asks it.
pub fn log_event_payload(
    record: &LogRecord,
) -> Option<(String, std::collections::HashMap<String, String>)> {
    let (name, payload, body) = recognised(record)?;
    let payload = match payload {
        LogEventPayload::BodyMembers => body,
        LogEventPayload::Attributes => structured_attributes(&record.attributes),
    };
    Some((name, payload))
}

/// The declaration that recognises a record: the event's name, where its attributes are, and the body's members.
fn recognised(
    record: &LogRecord,
) -> Option<(
    String,
    LogEventPayload,
    std::collections::HashMap<String, String>,
)> {
    let attributes = extract_attributes(&record.attributes);
    let event_name = (!record.event_name.is_empty()).then_some(record.event_name.as_str());
    // The body's members, read once: a declaration may name the event from one of them, and `body_members`
    // then reads the rest of them as the event's attributes. Both shapes, as that payload reads them.
    let body = body_members(record);
    let declared = sideseat_domain::rules::ruleset().log_events.recognise(
        event_name,
        |key| attributes.get(key).map(String::as_str),
        |member| body.get(member).map(String::as_str),
    )?;
    Some((declared.name.clone(), declared.payload, body))
}

/// The record's body as the members an event's attributes would be: a map as it stands, or text that parses as
/// a JSON object - a Python `logging` record's body is its message string, so a producer that logs an event
/// through `logging` serialises the members into it. A body that is neither has no members, which reads as an
/// event with no attributes, exactly as the same event emitted on a span with none would.
fn body_members(record: &LogRecord) -> std::collections::HashMap<String, String> {
    match record.body.as_ref().and_then(|body| body.value.as_ref()) {
        Some(any_value::Value::KvlistValue(members)) => structured_attributes(&members.values),
        Some(any_value::Value::StringValue(text)) => json_members(text),
        _ => Default::default(),
    }
}

/// The members of a JSON object written as text, each as the string an attribute of that value would be.
fn json_members(text: &str) -> std::collections::HashMap<String, String> {
    let Ok(serde_json::Value::Object(members)) = serde_json::from_str(text) else {
        return Default::default();
    };
    members
        .into_iter()
        .map(|(key, value)| {
            let value = match value {
                serde_json::Value::String(text) => text,
                other => other.to_string(),
            };
            (key, value)
        })
        .collect()
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
        let messages = log_record_messages(&record, None);
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
        let messages = log_record_messages(&record, None);
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
        let messages = log_record_messages(&record, None);
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
        assert!(
            log_record_messages(&unlinked, None).is_empty(),
            "no span ids"
        );
        let zero = LogRecord {
            trace_id: vec![0; 16],
            span_id: vec![0; 8],
            ..unlinked.clone()
        };
        assert!(
            log_record_messages(&zero, None).is_empty(),
            "invalid span ids"
        );
        let undeclared = linked(LogRecord {
            event_name: "app.checkout".to_string(),
            body,
            ..Default::default()
        });
        assert!(
            log_record_messages(&undeclared, None).is_empty(),
            "no declaration"
        );
    }

    /// Bytes in a message are content, read as base64: OpenTelemetry's botocore instrumentation writes the
    /// image and the PDF a user sent as byte values, and the generic hex conversion made them unreadable as
    /// the media they are.
    #[test]
    fn byte_content_is_base64() {
        let record = linked(LogRecord {
            event_name: "gen_ai.user.message".to_string(),
            body: Some(map(vec![kv(
                "content",
                AnyValue {
                    value: Some(any_value::Value::ArrayValue(ArrayValue {
                        values: vec![map(vec![kv(
                            "image",
                            map(vec![
                                kv("format", text("jpeg")),
                                kv(
                                    "source",
                                    map(vec![kv(
                                        "bytes",
                                        AnyValue {
                                            value: Some(any_value::Value::BytesValue(vec![
                                                0xff, 0xd8, 0xff,
                                            ])),
                                        },
                                    )]),
                                ),
                            ]),
                        )])],
                    })),
                },
            )])),
            ..Default::default()
        });
        let messages = log_record_messages(&record, None);
        assert_eq!(
            messages[0].content["content"][0]["image"]["source"]["bytes"],
            json!("/9j/"),
            "base64, not hex"
        );
    }

    /// A Python `logging` record's body is its message string, so an event logged through `logging` carries
    /// its members as JSON text. Semantic Kernel logs every prompt and completion this way.
    #[test]
    fn body_members_written_as_json_text_are_the_event_attributes() {
        let record = linked(LogRecord {
            attributes: vec![kv("event.name", text("gen_ai.choice"))],
            body: Some(text(
                r#"{"message": {"role": "assistant", "content": "Paris"}, "finish_reason": "stop"}"#,
            )),
            ..Default::default()
        });
        let messages = log_record_messages(&record, None);
        assert_eq!(sources(&messages), vec!["event:gen_ai.choice".to_string()]);
        assert_eq!(
            messages[0].content["message"],
            json!({"role": "assistant", "content": "Paris"})
        );
        assert_eq!(messages[0].content["finish_reason"], json!("stop"));
    }

    /// A record a framing carrier's messages are read from states the frame's key, read from its own payload; a
    /// record without the key, or one whose messages frame nothing, states none.
    #[test]
    fn a_frame_record_states_its_key() {
        let system_prompt = |extra: Vec<KeyValue>| {
            let mut attributes = vec![
                kv("event.name", text("system_prompt")),
                kv("system_prompt", text("You are a researcher.")),
                kv("system_prompt_length", text("21")),
            ];
            attributes.extend(extra);
            linked(LogRecord {
                attributes,
                ..Default::default()
            })
        };
        let keyed = system_prompt(vec![kv("system_prompt_hash", text("sp_a47e"))]);
        let messages = log_record_messages(&keyed, None);
        assert!(!messages.is_empty(), "the instruction is read");
        assert_eq!(
            log_record_frame_key(&keyed, &messages).as_deref(),
            Some("sp_a47e")
        );
        let unkeyed = system_prompt(Vec::new());
        assert_eq!(
            log_record_frame_key(&unkeyed, &log_record_messages(&unkeyed, None)),
            None,
            "a frame record without its key frames nothing"
        );
        let blank = system_prompt(vec![kv("system_prompt_hash", text(" "))]);
        assert_eq!(
            log_record_frame_key(&blank, &log_record_messages(&blank, None)),
            None,
            "nor with a blank one"
        );
        let prompt = linked(LogRecord {
            attributes: vec![
                kv("event.name", text("user_prompt")),
                kv("prompt", text("hi")),
                kv("system_prompt_hash", text("sp_a47e")),
            ],
            ..Default::default()
        });
        assert_eq!(
            log_record_frame_key(&prompt, &log_record_messages(&prompt, None)),
            None,
            "a record whose messages frame nothing states no frame key, whatever it carries"
        );
        // The record's key and a request span's are read through one renderer, from the value itself: a number is
        // a key on both sides alike, and a list or a map - which the two sides' general renderings write
        // differently - is a key on neither.
        let frames = sideseat_domain::rules::ruleset().carriers.frames();
        let both = |value: AnyValue| {
            let record = system_prompt(vec![kv("system_prompt_hash", value.clone())]);
            (
                log_record_frame_key(&record, &log_record_messages(&record, None)),
                crate::frame_keys::request_key(frames, &[kv("system_prompt_hash", value)]),
            )
        };
        let number = || AnyValue {
            value: Some(any_value::Value::IntValue(42)),
        };
        assert_eq!(both(number()), (Some("42".into()), Some("42".into())));
        let list = AnyValue {
            value: Some(any_value::Value::ArrayValue(ArrayValue {
                values: vec![number()],
            })),
        };
        assert_eq!(both(list), (None, None));
        let map = AnyValue {
            value: Some(any_value::Value::KvlistValue(KeyValueList {
                values: vec![kv("a", number())],
            })),
        };
        assert_eq!(both(map), (None, None));
    }

    /// A body's member states a key as an attribute of the same value would: from a map, or from JSON text.
    #[test]
    fn a_body_member_states_a_key_as_an_attribute_would() {
        let with_body = |body: AnyValue| LogRecord {
            body: Some(body),
            ..Default::default()
        };
        let members = with_body(AnyValue {
            value: Some(any_value::Value::KvlistValue(KeyValueList {
                values: vec![
                    kv(
                        "key",
                        AnyValue {
                            value: Some(any_value::Value::IntValue(42)),
                        },
                    ),
                    kv("blank", text(" ")),
                ],
            })),
        });
        assert_eq!(body_member_key(&members, "key").as_deref(), Some("42"));
        assert_eq!(body_member_key(&members, "blank"), None);
        assert_eq!(body_member_key(&members, "absent"), None);
        let json = with_body(text(r#"{"key": 42, "list": [42], "text": "sp_1"}"#));
        assert_eq!(body_member_key(&json, "key").as_deref(), Some("42"));
        assert_eq!(body_member_key(&json, "text").as_deref(), Some("sp_1"));
        assert_eq!(body_member_key(&json, "list"), None);
        assert_eq!(body_member_key(&with_body(text("not json")), "key"), None);
    }
}
