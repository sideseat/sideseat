//! The one reader for message events, whether a span event or a log record carried them.
//!
//! Both carriers say the same thing - an event name, its attributes, an instant - so both are read by the
//! same declared `message_events` entry and readings. Only the span context differs: a span event is read
//! beside the span it belongs to, while a log record is read on its own, before the span it names may have
//! arrived.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use serde_json::{Value as JsonValue, json};
use sideseat_domain::observations::RawMessage;

use crate::otlp::{any_value_to_json, any_value_to_string};

/// What a rule may ask about the span an event belongs to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EventSpan<'a> {
    pub name: &'a str,
    pub attrs: &'a HashMap<String, String>,
    pub is_tool_span: bool,
}

impl EventSpan<'static> {
    /// No span context: the reading a log record gets at ingest.
    ///
    /// Conservative by construction - a gate on the span's name or attributes cannot hold, and the event is
    /// not read as a tool span's - because the span may not have arrived and its row is not consulted.
    pub(crate) fn unattached() -> Self {
        static EMPTY: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
        Self {
            name: "",
            attrs: EMPTY.get_or_init(HashMap::new),
            is_tool_span: false,
        }
    }
}

/// Whether this event carries messages at all.
///
/// Declared (`message_events`), not a list here: which events a producer writes messages on is the same kind
/// of fact as which attributes it writes them on. As a Rust list it also made a new `when_event` rule a
/// valid but *dead* declaration - the rule compiled, and the event was rejected before the plan was asked.
pub(crate) fn is_message_event(event_name: &str) -> bool {
    sideseat_domain::rules::ruleset()
        .message_events
        .contains_key(event_name)
}

/// Read one message event: its raw form and whatever the declared readings make of it.
///
/// The span is passed as well as the event, because a rule's gate asks about the span while its `read` draws
/// from the event. With only the event, a `span_name` gate compiled and could never hold.
pub(crate) fn read_message_event(
    name: &str,
    attrs: &HashMap<String, String>,
    time: DateTime<Utc>,
    span: EventSpan<'_>,
) -> Vec<RawMessage> {
    if !is_message_event(name) {
        return vec![];
    }

    // What the assets declare about *this* event, read from its own attributes. `replaces` says whether the
    // event's raw form is a message as well: a container event's attributes *are* the messages inside it, so
    // emitting the container too would report the conversation twice, while an event carrying a reply and a
    // bundled tool result wants both.
    let reading = sideseat_domain::rules::ruleset().messages.from_event(
        name,
        attrs,
        span.name,
        span.attrs,
        span.is_tool_span,
    );
    // A declared container that **nothing read** keeps its raw form, and says so. Suppressing it produced no
    // messages and no record: the event was indistinguishable from one never emitted, which for a container
    // whose payload failed to parse is the difference between "this span said nothing" and "this span said
    // something we could not read".
    if reading.unhandled_container {
        tracing::warn!(
            event = %name,
            span = %span.name,
            "event declares its raw form is a container, and no declared reading produced anything from it - \
             keeping the raw form rather than dropping the event"
        );
    }
    let declared: Vec<RawMessage> = reading
        .emissions
        .into_iter()
        .map(|emission| RawMessage::from_event(emission.carrier.name(), time, emission.value))
        .collect();
    if reading.replaces_raw {
        return declared;
    }

    // The raw message preserves the literal attributes only, no metadata. Role derivation happens at read
    // time (`sideml/normalize.rs`), which is where tool-span semantics are applied.
    // In key order, so one event always serialises to the same bytes: the attributes arrive as a hash map,
    // and a stored message whose member order varied between two deliveries of one record would differ
    // byte-for-byte while saying the same thing.
    let mut keyed: Vec<(&String, &String)> = attrs.iter().collect();
    keyed.sort_unstable();
    let mut raw = serde_json::Map::new();
    for (key, value) in keyed {
        let json_val = if value.starts_with('{') || value.starts_with('[') {
            parse_json_with_fallback(value, &format!("event.{name}.{key}"))
        } else {
            json!(value)
        };
        raw.insert(key.clone(), json_val);
    }

    let mut messages = vec![RawMessage::from_event(name, time, JsonValue::Object(raw))];
    // Whatever the assets said about this event, beside its raw form.
    messages.extend(declared);
    messages
}

/// Attributes as the event reader takes them, with structured values kept whole.
///
/// A span event's attributes are scalars or JSON text in practice, and `extract_attributes` renders them
/// that way. A log record's are not: its body members and attributes are typed `AnyValue` maps and arrays,
/// and the generic rendering stringifies every nested value, so `{"message": {"role": ...}}` would arrive
/// as a map of strings. Structured values are therefore rendered as JSON text, which the reader parses
/// back into exactly the structure the producer sent.
pub(crate) fn structured_attributes(attrs: &[KeyValue]) -> HashMap<String, String> {
    attrs
        .iter()
        .filter_map(|kv| {
            kv.value
                .as_ref()
                .map(|value| (kv.key.clone(), structured_value(value)))
        })
        .collect()
}

fn structured_value(value: &AnyValue) -> String {
    match &value.value {
        Some(any_value::Value::ArrayValue(_) | any_value::Value::KvlistValue(_)) => {
            serde_json::to_string(&any_value_to_json(value)).unwrap_or_default()
        }
        _ => any_value_to_string(value),
    }
}

/// Parse a string as JSON, with logging on parse failure.
///
/// Returns the parsed JSON value on success, or the original string as a JSON string on failure.
pub(crate) fn parse_json_with_fallback(value: &str, context: &str) -> JsonValue {
    match serde_json::from_str(value) {
        Ok(json) => json,
        Err(e) => {
            tracing::trace!(
                context = context,
                error = %e,
                value_preview = %truncate_for_log(value, 100),
                "JSON parse failed, using string fallback"
            );
            json!(value)
        }
    }
}

/// Truncate a string for logging purposes (UTF-8 safe).
fn truncate_for_log(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{ArrayValue, KeyValueList};

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
}
