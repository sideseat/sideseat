//! Messages carried by log records that name a span: the one derivation, read at ingest from the record and at
//! read time from its raw record (`log_record`).
//!
//! Some instrumentations report the conversation as log records linked to a span rather than as span events. A
//! record a declared `log_events` entry recognises has its messages read by the reader span events use
//! (`message_events`). They are joined to the span at read time and never written into the span's row: span rows
//! are latest-ingest-wins and may arrive after the log, and the log is durable before acknowledgement while the
//! span may still be queued. Ingest stores them only where the raw record does not rebuild them; a read rebuilds
//! the rest here ([`rebuild_messages`]).

use std::collections::HashMap;

use base64::prelude::*;
use serde_json::{Map, Value};
use sideseat_core::utils::time::nanos_to_datetime;

use crate::log_record::{
    LogRecordContent, OtlpValue, from_raw, notes_for, rewrite_values, to_json, to_text,
};
use crate::message_events::{EventSpan, read_message_event};
use crate::observations::RawMessage;
use crate::rules::schema::LogEventPayload;
use sideseat_ports::types::{
    LogMessageSource, MessageSpanRow, RequestFrameRecord, noted_references_from_column,
};

/// The raw messages one log record carries, or none.
///
/// Nothing is read from a record that names no span, since nothing could attach it to a conversation, nor from
/// one no declaration recognises. The instant is the record's own time, falling back to its observed time - never
/// the receipt time, so a re-delivered record reads exactly as it did the first time.
pub fn log_record_messages(record: &LogRecordContent, scope: Option<&str>) -> Vec<RawMessage> {
    if !record.names_a_span {
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

/// The messages as the column holds them: their JSON array, each noted reference replaced where it lands - where a
/// value decoded out of the record's text (JSON spelling `#` as `\u0023`, say) first appears. `None` for none.
pub fn messages_text(messages: &[RawMessage], notes: &[(String, String)]) -> Option<String> {
    (!messages.is_empty()).then(|| {
        let mut value = serde_json::to_value(messages).unwrap_or_default();
        rewrite_values(&mut value, notes);
        value.to_string()
    })
}

/// The messages a stored raw record rebuilds, as the column holds them: `Some(None)` for a record that carries
/// none, and `None` where the raw record does not decode.
pub fn rebuild_messages(
    raw_log: &str,
    scope: Option<&str>,
    noted: &[String],
) -> Option<Option<String>> {
    let notes = notes_for(noted);
    let record = from_raw(raw_log, &notes)?;
    Some(messages_text(&log_record_messages(&record, scope), &notes))
}

/// One record's messages from the store's answer: the stored array, or the one its raw record rebuilds. `None`
/// for a raw record that does not rebuild - which ingest checked it does before leaving the messages unstored.
pub fn resolve_source(source: &LogMessageSource) -> Option<String> {
    if let Some(messages) = &source.messages {
        return Some(messages.clone());
    }
    let noted = noted_references_from_column(source.noted_references.as_deref());
    let rebuilt = rebuild_messages(
        source.raw_log.as_deref()?,
        source.scope_name.as_deref(),
        &noted,
    );
    if rebuilt.is_none() {
        tracing::error!(
            "a log record's raw form no longer rebuilds the messages ingest left unstored"
        );
    }
    rebuilt.flatten()
}

/// A span's log records' messages, concatenated in the order the store answered them into one array, as the log
/// aggregate writes them: each record's array without its brackets, joined. `[]` for none.
pub fn concatenated(sources: &[LogMessageSource]) -> String {
    let parts: Vec<String> = sources
        .iter()
        .filter_map(resolve_source)
        .map(|messages| {
            messages
                .strip_prefix('[')
                .and_then(|inner| inner.strip_suffix(']'))
                .unwrap_or(&messages)
                .to_string()
        })
        .collect();
    if parts.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", parts.join(","))
    }
}

/// A message row with its log records' messages resolved from the store's answer.
pub fn resolve_row(row: &mut MessageSpanRow) {
    if !row.log_message_sources.is_empty() {
        row.log_messages_json = concatenated(&std::mem::take(&mut row.log_message_sources));
    }
}

/// A frame record with its messages resolved from the store's answer.
pub fn resolve_frame(record: &mut RequestFrameRecord) {
    if let Some(source) = record.source.take() {
        record.messages_json = resolve_source(&source);
    }
}

/// The message event a log record is declared to carry: its name, and the attributes the event reader takes - the
/// record's body members or its own attributes, as the `log_events` declaration says. `None` when no declaration
/// recognises the record.
pub fn log_event_payload(record: &LogRecordContent) -> Option<(String, HashMap<String, String>)> {
    let (name, payload, body) = recognised(record)?;
    let payload = match payload {
        LogEventPayload::BodyMembers => body,
        LogEventPayload::Attributes => structured_attributes(&record.attributes),
    };
    Some((name, payload))
}

/// The declaration that recognises a record: the event's name, where its attributes are, and the body's members.
fn recognised(
    record: &LogRecordContent,
) -> Option<(String, LogEventPayload, HashMap<String, String>)> {
    // Each attribute as its text, a pair without a value left out; a repeated key's last value wins.
    let attributes: HashMap<&str, String> = record
        .attributes
        .iter()
        .filter_map(|(key, value)| Some((key.as_str(), to_text(value.as_ref()?))))
        .collect();
    let event_name = (!record.event_name.is_empty()).then_some(record.event_name.as_str());
    // The body's members, read once: a declaration may name the event from one of them, and `body_members` then
    // reads the rest of them as the event's attributes. Both shapes, as that payload reads them.
    let body = body_members(record);
    let declared = crate::rules::ruleset().log_events.recognise(
        event_name,
        |key| attributes.get(key).map(String::as_str),
        |member| body.get(member).map(String::as_str),
    )?;
    Some((declared.name.clone(), declared.payload, body))
}

/// The record's body as the members an event's attributes would be: a map as it stands, or text that parses as a
/// JSON object - a Python `logging` record's body is its message string, so a producer that logs an event through
/// `logging` serialises the members into it. A body that is neither has no members, which reads as an event with
/// no attributes, exactly as the same event emitted on a span with none would.
fn body_members(record: &LogRecordContent) -> HashMap<String, String> {
    match &record.body {
        Some(OtlpValue::Kvlist(members)) => structured_attributes(members),
        Some(OtlpValue::String(text)) => json_members(text),
        _ => HashMap::new(),
    }
}

/// Attributes as the event reader takes them, with structured values kept whole.
///
/// A log record's body members and attributes are typed maps and arrays, and the generic rendering stringifies
/// every nested value, so `{"message": {"role": ...}}` would arrive as a map of strings. Structured values are
/// therefore rendered as JSON text, which the reader parses back into exactly the structure the producer sent. A
/// member with no value is left out, whether the exporter omitted the value or sent an empty one: OpenTelemetry
/// Python writes an unset field one way or the other depending on its release, and the two must read alike.
fn structured_attributes(pairs: &[(String, Option<OtlpValue>)]) -> HashMap<String, String> {
    pairs
        .iter()
        .filter_map(|(key, value)| {
            let value = value.as_ref().filter(|value| **value != OtlpValue::Empty)?;
            Some((key.clone(), structured_value(value)))
        })
        .collect()
}

fn structured_value(value: &OtlpValue) -> String {
    match value {
        OtlpValue::Array(_) | OtlpValue::Kvlist(_) => {
            serde_json::to_string(&content_json(value)).unwrap_or_default()
        }
        _ => to_text(value),
    }
}

/// A structured event value as JSON, with bytes as base64.
///
/// Bytes inside a message are content - an image or a document a user sent - and base64 is how every content
/// block carries binary data. The generic conversion writes bytes as hex, which suits an identifier and made such a
/// payload unreadable as the media it is.
fn content_json(value: &OtlpValue) -> Value {
    match value {
        OtlpValue::Bytes(bytes) => Value::String(BASE64_STANDARD.encode(bytes)),
        OtlpValue::Array(items) => Value::Array(items.iter().map(content_json).collect()),
        // A member with no value is left out, as at the top level: omitted and empty read alike.
        OtlpValue::Kvlist(members) => Value::Object(
            members
                .iter()
                .filter_map(|(key, value)| {
                    let value = value.as_ref().filter(|value| **value != OtlpValue::Empty)?;
                    Some((key.clone(), content_json(value)))
                })
                .collect::<Map<String, Value>>(),
        ),
        _ => to_json(value),
    }
}

/// The members of a JSON object written as text, each as the string an attribute of that value would be.
fn json_members(text: &str) -> HashMap<String, String> {
    let Ok(Value::Object(members)) = serde_json::from_str(text) else {
        return HashMap::new();
    };
    // A null member, at any depth, is one the record left unset: OTLP's AnyValue has no null, so the structured
    // encoding of the same body writes an unset member with an empty value, which is left out - the text encoding
    // must read alike or the two reconstruct differently. A null array *element* stays, as the structured
    // encoding keeps it.
    members
        .into_iter()
        .filter_map(|(key, value)| {
            let value = match value {
                Value::Null => return None,
                Value::String(text) => text,
                other => without_null_members(other).to_string(),
            };
            Some((key, value))
        })
        .collect()
}

/// The value with every object member that is null removed, at any depth.
fn without_null_members(value: Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .into_iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(key, member)| (key, without_null_members(member)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(without_null_members).collect()),
        other => other,
    }
}
