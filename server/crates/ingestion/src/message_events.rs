//! The one reader for message events, whether a span event or a log record carried them.
//!
//! Both carriers say the same thing - an event name, its attributes, an instant - so both are read by the
//! same declared `message_events` entry and readings. Only the span context differs: a span event is read
//! beside the span it belongs to, while a log record is read on its own, before the span it names may have
//! arrived.

use std::collections::HashMap;

use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use serde_json::{Value as JsonValue, json};

use crate::otlp::{any_value_to_json, any_value_to_string};

/// Attributes as the event reader takes them, with structured values kept whole.
///
/// A span event's attributes are scalars or JSON text in practice, and `extract_attributes` renders them
/// that way. A log record's are not: its body members and attributes are typed `AnyValue` maps and arrays,
/// and the generic rendering stringifies every nested value, so `{"message": {"role": ...}}` would arrive
/// as a map of strings. Structured values are therefore rendered as JSON text, which the reader parses
/// back into exactly the structure the producer sent.
/// A span event's attributes as the event reader sees them: rendered as text, with a member whose value is
/// empty left out, as [`structured_attributes`] leaves it - an exporter writes an unset field with no value
/// or an empty one, and both must read as the member being absent.
pub(crate) fn event_attributes(attrs: &[KeyValue]) -> HashMap<String, String> {
    attrs
        .iter()
        .filter_map(|kv| {
            kv.value
                .as_ref()
                .filter(|value| value.value.is_some())
                .map(|value| (kv.key.clone(), any_value_to_string(value)))
        })
        .collect()
}

pub(crate) fn structured_attributes(attrs: &[KeyValue]) -> HashMap<String, String> {
    attrs
        .iter()
        // A member with no value is left out, whether the exporter omitted the value or sent an empty one:
        // OpenTelemetry Python writes an unset field one way or the other depending on its release, and the two
        // must read alike. Read as text, an empty value would be a member set to nothing.
        .filter_map(|kv| {
            kv.value
                .as_ref()
                .filter(|value| value.value.is_some())
                .map(|value| (kv.key.clone(), structured_value(value)))
        })
        .collect()
}

fn structured_value(value: &AnyValue) -> String {
    match &value.value {
        Some(any_value::Value::ArrayValue(_) | any_value::Value::KvlistValue(_)) => {
            serde_json::to_string(&content_json(value)).unwrap_or_default()
        }
        _ => any_value_to_string(value),
    }
}

/// A structured event value as JSON, with bytes as base64.
///
/// Bytes inside a message are content - an image or a document a user sent - and base64 is how every
/// content block carries binary data. The generic conversion writes bytes as hex, which suits an identifier
/// and made such a payload unreadable as the media it is.
fn content_json(value: &AnyValue) -> JsonValue {
    use base64::Engine as _;
    match &value.value {
        Some(any_value::Value::BytesValue(bytes)) => {
            json!(base64::engine::general_purpose::STANDARD.encode(bytes))
        }
        Some(any_value::Value::ArrayValue(array)) => {
            JsonValue::Array(array.values.iter().map(content_json).collect())
        }
        // A member with no value is left out, as at the top level: omitted and empty read alike.
        Some(any_value::Value::KvlistValue(members)) => JsonValue::Object(
            members
                .values
                .iter()
                .filter_map(|kv| {
                    kv.value
                        .as_ref()
                        .filter(|v| v.value.is_some())
                        .map(|v| (kv.key.clone(), content_json(v)))
                })
                .collect(),
        ),
        _ => any_value_to_json(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{ArrayValue, KeyValueList};
    use sideseat_domain::message_events::{EventSpan, read_message_event};
    use sideseat_domain::observations::RawMessage;

    fn text(value: &str) -> AnyValue {
        AnyValue {
            value: Some(any_value::Value::StringValue(value.to_string())),
        }
    }

    #[test]
    fn structured_values_survive_as_json_text() {
        let message = AnyValue {
            value: Some(any_value::Value::KvlistValue(KeyValueList {
                values: vec![
                    KeyValue {
                        key: "role".to_string(),
                        value: Some(text("assistant")),
                    },
                    KeyValue {
                        key: "content".to_string(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::ArrayValue(ArrayValue {
                                values: vec![AnyValue {
                                    value: Some(any_value::Value::KvlistValue(KeyValueList {
                                        values: vec![KeyValue {
                                            key: "text".to_string(),
                                            value: Some(text("hi")),
                                        }],
                                    })),
                                }],
                            })),
                        }),
                    },
                ],
            })),
        };
        let attrs = structured_attributes(&[
            KeyValue {
                key: "message".to_string(),
                value: Some(message),
            },
            KeyValue {
                key: "index".to_string(),
                value: Some(AnyValue {
                    value: Some(any_value::Value::IntValue(0)),
                }),
            },
        ]);
        assert_eq!(attrs["index"], "0");
        let parsed: JsonValue = serde_json::from_str(&attrs["message"]).unwrap();
        assert_eq!(
            parsed,
            json!({"role": "assistant", "content": [{"text": "hi"}]}),
            "nested members keep their structure rather than becoming strings"
        );
    }

    fn names(messages: &[RawMessage]) -> Vec<String> {
        messages
            .iter()
            .map(|m| match &m.source {
                sideseat_domain::observations::MessageSource::Event { name, .. } => name.clone(),
                sideseat_domain::observations::MessageSource::Attribute { key, .. } => key.clone(),
            })
            .collect()
    }

    /// A reading that owns the attribute holding the message leaves the raw form with no content, so the
    /// event is reported once, as the reading; one that owns nothing keeps the raw form whole.
    #[test]
    fn the_raw_form_never_repeats_what_a_reading_owns() {
        let time = chrono::Utc::now();
        let read = HashMap::from([
            (
                "content".to_string(),
                r#"{"role":"model","parts":[{"text":"Visit the Prado."}]}"#.to_string(),
            ),
            ("finish_reason".to_string(), "STOP".to_string()),
            ("index".to_string(), "0".to_string()),
        ]);
        let messages = read_message_event(
            "gen_ai.choice",
            &read,
            time,
            EventSpan::unattached(Some("gcp.vertex.agent")),
        );
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_ne!(names(&messages), vec!["gen_ai.choice".to_string()]);
        assert_eq!(messages[0].content["parts"][0]["text"], "Visit the Prado.");
        assert_eq!(
            messages[0].content["finish_reason"], "STOP",
            "the finish reason beside the message is lifted onto it"
        );
        // Another scope's event of the same shape is not ADK's, so nothing reads it.
        let other = read_message_event(
            "gen_ai.choice",
            &read,
            time,
            EventSpan::unattached(Some("another.scope")),
        );
        assert_eq!(names(&other), vec!["gen_ai.choice".to_string()]);

        // `content` that is not the model API's message is the event's ordinary member.
        let plain = HashMap::from([("content".to_string(), "Visit the Prado.".to_string())]);
        let messages = read_message_event(
            "gen_ai.choice",
            &plain,
            time,
            EventSpan::unattached(Some("gcp.vertex.agent")),
        );
        assert_eq!(names(&messages), vec!["gen_ai.choice".to_string()]);
        assert_eq!(messages[0].content["content"], "Visit the Prado.");
    }

    /// The choice that only ends a streamed answer - a finish reason, and a `content` member with no value, which
    /// the event does not have - is read as the stream's terminal, holding nothing but why the model stopped.
    #[test]
    fn a_contentless_finish_choice_ends_the_stream() {
        // A member with no value, whichever way the exporter wrote it.
        for content in [None, Some(AnyValue { value: None })] {
            let body = structured_attributes(&[
                KeyValue {
                    key: "content".to_string(),
                    value: content,
                },
                KeyValue {
                    key: "index".to_string(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::IntValue(0)),
                    }),
                },
                KeyValue {
                    key: "finish_reason".to_string(),
                    value: Some(text("STOP")),
                },
            ]);
            let messages = read_message_event(
                "gen_ai.choice",
                &body,
                chrono::Utc::now(),
                EventSpan::unattached(Some("gcp.vertex.agent")),
            );
            let readings: Vec<&RawMessage> =
                messages.iter().filter(|m| m.stream.is_some()).collect();
            assert_eq!(readings.len(), 1, "{messages:?}");
            assert_eq!(
                readings[0].stream,
                Some(sideseat_domain::rules::schema::StreamMark::Delta)
            );
            assert_eq!(
                readings[0].content,
                json!({"role": "model", "parts": [], "finish_reason": "STOP"})
            );
        }
    }

    /// A message's member with no value is left out however deep it sits, whichever way the exporter wrote it:
    /// an empty value is a member left unset, never one set to nothing.
    #[test]
    fn an_empty_member_value_is_a_member_left_unset() {
        let part = |unset: Option<AnyValue>| AnyValue {
            value: Some(any_value::Value::KvlistValue(KeyValueList {
                values: vec![
                    KeyValue {
                        key: "text".to_string(),
                        value: Some(text("hello")),
                    },
                    KeyValue {
                        key: "function_response".to_string(),
                        value: unset,
                    },
                ],
            })),
        };
        let read = |unset: Option<AnyValue>| {
            let attrs = structured_attributes(&[KeyValue {
                key: "content".to_string(),
                value: Some(part(unset)),
            }]);
            serde_json::from_str::<JsonValue>(&attrs["content"]).expect("structured content")
        };
        assert_eq!(read(None), json!({"text": "hello"}));
        assert_eq!(read(Some(AnyValue { value: None })), read(None));
    }

    /// A chunk's call is settled unless it carries a streaming member - `will_continue`, true or false, or
    /// `partial_args`, even the empty list a call opens with - each on its own. A member with no value is none.
    #[test]
    fn a_chunk_s_call_is_settled_unless_it_carries_a_streaming_member() {
        use sideseat_domain::rules::schema::StreamMark;
        let chunk = |call: serde_json::Value| {
            let content = json!({"role": "model", "parts": [{"function_call": call}]});
            let attrs = HashMap::from([
                ("content".to_string(), content.to_string()),
                ("index".to_string(), "0".to_string()),
            ]);
            let messages = read_message_event(
                "gen_ai.choice",
                &attrs,
                chrono::Utc::now(),
                EventSpan::unattached(Some("gcp.vertex.agent")),
            );
            messages.iter().find_map(|m| m.stream)
        };
        let base = json!({"id": "c1", "name": "get_weather", "args": {"city": "Milan"}});
        let with = |member: &str, value: serde_json::Value| {
            let mut call = base.clone();
            call[member] = value;
            call
        };
        let partial = json!([{"json_path": "$.city", "string_value": "Mi"}]);
        for (call, mark) in [
            (base.clone(), StreamMark::SettledChunk),
            (
                with("will_continue", JsonValue::Null),
                StreamMark::SettledChunk,
            ),
            (
                with("partial_args", JsonValue::Null),
                StreamMark::SettledChunk,
            ),
            (with("will_continue", json!(true)), StreamMark::Chunk),
            (with("will_continue", json!(false)), StreamMark::Chunk),
            (with("partial_args", partial), StreamMark::Chunk),
            (with("partial_args", json!([])), StreamMark::Chunk),
        ] {
            assert_eq!(chunk(call.clone()), Some(mark), "{call}");
        }
    }
}
