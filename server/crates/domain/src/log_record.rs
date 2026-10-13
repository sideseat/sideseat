//! A log record's content as its views and messages are derived from it, decoded either from the stored raw record
//! or, at ingest, from the record itself.
//!
//! One decoded form, so every derivation is one function whichever form it is read from: the body views and the
//! attributes (`log_views`) and the messages the record carries (`log_messages`). The raw record is read from its
//! text - each number from its own digits - with every producer string unescaped, each media value SideSeat stored
//! read as its reference, and each noted reference shown as its note.

use std::collections::HashMap;

use base64::prelude::*;
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use sideseat_core::utils::file_uri::{is_file_uri, unescape_producer_text};

use crate::files::{missing_content_note, replace_file_reference};

/// An OTLP `AnyValue`, decoded.
#[derive(Debug, Clone, PartialEq)]
pub enum OtlpValue {
    /// A value with no kind set.
    Empty,
    String(String),
    Bool(bool),
    Int(i64),
    Double(f64),
    Bytes(Vec<u8>),
    Array(Vec<OtlpValue>),
    /// In the producer's order; a member without a value is `None`.
    Kvlist(Vec<(String, Option<OtlpValue>)>),
}

/// The parts of a log record its views and messages are derived from.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LogRecordContent {
    /// Whether the record carries a usable trace and span id; all-zero ids are OpenTelemetry's "invalid".
    pub names_a_span: bool,
    pub time_unix_nano: u64,
    pub observed_time_unix_nano: u64,
    pub event_name: String,
    /// In the producer's order; a pair without a value is `None`.
    pub attributes: Vec<(String, Option<OtlpValue>)>,
    /// `None` for a record without a body.
    pub body: Option<OtlpValue>,
}

/// Each noted reference with the note a view shows in its place.
pub fn notes_for(noted: &[String]) -> Vec<(String, String)> {
    noted
        .iter()
        .map(|uri| (uri.clone(), missing_content_note(uri)))
        .collect()
}

/// A stored raw record decoded, with these notes applied to its string values. `None` where it is not one this
/// decoding reads - not JSON, or a value of no kind it knows.
pub fn from_raw(raw_log: &str, notes: &[(String, String)]) -> Option<LogRecordContent> {
    let record = object(serde_json::from_str::<&RawValue>(raw_log).ok()?)?;
    // The ids as the record's OTLP/JSON writes them, hex; an absent or all-zero one names nothing.
    let id_is_valid = |member: &str| -> Option<bool> {
        match present(record.get(member).copied()) {
            None => Some(false),
            Some(id) => {
                let id = string(id)?;
                Some(!id.is_empty() && id.bytes().any(|digit| digit != b'0'))
            }
        }
    };
    let instant = |member: &str| -> Option<u64> {
        match present(record.get(member).copied()) {
            None => Some(0),
            Some(raw) if raw.get().starts_with('"') => string(raw)?.parse().ok(),
            Some(raw) => raw.get().parse().ok(),
        }
    };
    Some(LogRecordContent {
        names_a_span: id_is_valid("traceId")? && id_is_valid("spanId")?,
        time_unix_nano: instant("timeUnixNano")?,
        observed_time_unix_nano: instant("observedTimeUnixNano")?,
        event_name: match present(record.get("eventName").copied()) {
            None => String::new(),
            Some(name) => unescape(&string(name)?),
        },
        attributes: decode_pairs(record.get("attributes").copied(), notes)?,
        body: match present(record.get("body").copied()) {
            None => None,
            Some(body) => Some(decode(body, notes)?),
        },
    })
}

/// The members of a JSON object, each left as its text: a number is read from its own digits, exactly, rather than
/// through a parse that may round it a bit away from the value ingest rendered.
fn object(raw: &RawValue) -> Option<HashMap<String, &RawValue>> {
    serde_json::from_str(raw.get()).ok()
}

/// A member's value unless it is absent or `null`.
fn present(raw: Option<&RawValue>) -> Option<&RawValue> {
    raw.filter(|raw| raw.get() != "null")
}

fn string(raw: &RawValue) -> Option<String> {
    serde_json::from_str(raw.get()).ok()
}

fn list(raw: &RawValue) -> Option<Vec<&RawValue>> {
    serde_json::from_str(raw.get()).ok()
}

fn decode(value: &RawValue, notes: &[(String, String)]) -> Option<OtlpValue> {
    let members = object(value)?;
    if let Some(text) = members.get("stringValue") {
        return Some(OtlpValue::String(noted_text(&string(text)?, notes)));
    }
    if let Some(flag) = members.get("boolValue") {
        return serde_json::from_str(flag.get()).ok().map(OtlpValue::Bool);
    }
    if let Some(number) = members.get("intValue") {
        let text = number.get();
        return Some(OtlpValue::Int(if text.starts_with('"') {
            string(number)?.parse().ok()?
        } else {
            text.parse().ok()?
        }));
    }
    if let Some(number) = members.get("doubleValue") {
        // From its digits, as Rust's correctly rounded parse reads them: the shortest text a double renders to
        // parses back to the same double.
        return number.get().parse().ok().map(OtlpValue::Double);
    }
    if let Some(bytes) = members.get("bytesValue") {
        let text = string(bytes)?;
        // A media value SideSeat stored reads as its reference, a string, as the decoded record holds it.
        if is_file_uri(&text) {
            return Some(OtlpValue::String(noted_text(&text, notes)));
        }
        return Some(OtlpValue::Bytes(BASE64_STANDARD.decode(text).ok()?));
    }
    if let Some(array) = members.get("arrayValue") {
        let items = match present(object(array)?.get("values").copied()) {
            None => Vec::new(),
            Some(values) => list(values)?,
        };
        return items
            .into_iter()
            .map(|item| decode(item, notes))
            .collect::<Option<Vec<_>>>()
            .map(OtlpValue::Array);
    }
    if let Some(kvlist) = members.get("kvlistValue") {
        return decode_pairs(object(kvlist)?.get("values").copied(), notes).map(OtlpValue::Kvlist);
    }
    members.is_empty().then_some(OtlpValue::Empty)
}

/// A string value as the producer wrote it, each noted reference replaced by its note.
fn noted_text(text: &str, notes: &[(String, String)]) -> String {
    let mut text = unescape(text);
    for (uri, note) in notes {
        if let Some(rewritten) = replace_file_reference(&text, uri, note) {
            text = rewritten;
        }
    }
    text
}

/// A list of OTLP/JSON key-value pairs - a key-value list's `values`, or a record's `attributes` - in order, each key
/// unescaped and each value decoded, `None` where a pair holds none. An absent list is empty.
fn decode_pairs(
    pairs: Option<&RawValue>,
    notes: &[(String, String)],
) -> Option<Vec<(String, Option<OtlpValue>)>> {
    let Some(pairs) = present(pairs) else {
        return Some(Vec::new());
    };
    let pairs = list(pairs)?;
    let mut decoded = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let pair = object(pair)?;
        let key = match pair.get("key") {
            None => String::new(),
            Some(key) => unescape(&string(key)?),
        };
        let value = match present(pair.get("value").copied()) {
            None => None,
            Some(value) => Some(decode(value, notes)?),
        };
        decoded.push((key, value));
    }
    Some(decoded)
}

fn unescape(text: &str) -> String {
    unescape_producer_text(text).into_owned()
}

pub fn to_json(value: &OtlpValue) -> Value {
    match value {
        OtlpValue::Empty => Value::Null,
        OtlpValue::String(text) => Value::String(text.clone()),
        OtlpValue::Bool(flag) => Value::Bool(*flag),
        OtlpValue::Int(number) => Value::from(*number),
        OtlpValue::Double(number) => {
            serde_json::Number::from_f64(*number).map_or(Value::Null, Value::Number)
        }
        OtlpValue::Bytes(bytes) => Value::String(hex::encode(bytes)),
        OtlpValue::Array(items) => Value::Array(items.iter().map(to_json).collect()),
        OtlpValue::Kvlist(pairs) => Value::Object(
            pairs
                .iter()
                .filter_map(|(key, value)| Some((key.clone(), to_json(value.as_ref()?))))
                .collect::<Map<String, Value>>(),
        ),
    }
}

/// An attribute value as ingest types it (`attrs_to_typed_json`): JSON's own kinds as themselves, an absent value
/// as `null`. `None` for bytes and a double JSON cannot hold, which ingest tags: a record holding either keeps its
/// views stored rather than this rendering learn the tag.
pub fn to_typed(value: &OtlpValue) -> Option<Value> {
    Some(match value {
        OtlpValue::Empty => Value::Null,
        OtlpValue::String(text) => Value::String(text.clone()),
        OtlpValue::Bool(flag) => Value::Bool(*flag),
        OtlpValue::Int(number) => Value::from(*number),
        OtlpValue::Double(number) => Value::Number(serde_json::Number::from_f64(*number)?),
        OtlpValue::Bytes(_) => return None,
        OtlpValue::Array(items) => Value::Array(items.iter().map(to_typed).collect::<Option<_>>()?),
        OtlpValue::Kvlist(pairs) => {
            let mut nested = Map::with_capacity(pairs.len());
            for (key, value) in pairs {
                nested.insert(
                    key.clone(),
                    value.as_ref().map_or(Some(Value::Null), to_typed)?,
                );
            }
            Value::Object(nested)
        }
    })
}

pub fn to_text(value: &OtlpValue) -> String {
    match value {
        OtlpValue::Empty => String::new(),
        OtlpValue::String(text) => text.clone(),
        OtlpValue::Bool(flag) => flag.to_string(),
        OtlpValue::Int(number) => number.to_string(),
        OtlpValue::Double(number) => number.to_string(),
        OtlpValue::Bytes(bytes) => hex::encode(bytes),
        OtlpValue::Array(items) => {
            let values: Vec<String> = items.iter().map(to_text).collect();
            serde_json::to_string(&values).unwrap_or_default()
        }
        OtlpValue::Kvlist(members) => {
            let mut pairs: Vec<(String, String)> = Vec::new();
            let mut position: HashMap<&str, usize> = HashMap::new();
            for (key, value) in members {
                let Some(value) = value else {
                    continue;
                };
                let rendered = to_text(value);
                match position.get(key.as_str()) {
                    Some(&index) => pairs[index].1 = rendered,
                    None => {
                        position.insert(key.as_str(), pairs.len());
                        pairs.push((key.clone(), rendered));
                    }
                }
            }
            serde_json::to_string(&InOrder(&pairs)).unwrap_or_default()
        }
    }
}

/// Key-value pairs serialised as a JSON object in their own order.
struct InOrder<'a>(&'a [(String, String)]);

impl serde::Serialize for InOrder<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().map(|(key, value)| (key, value)))
    }
}

/// Each note in every string value of a view's JSON, as ingest applies them after deriving it.
pub fn rewrite_values(value: &mut Value, notes: &[(String, String)]) {
    if notes.is_empty() {
        return;
    }
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::String(text) => {
                for (uri, note) in notes {
                    if let Some(rewritten) = replace_file_reference(text, uri, note) {
                        *text = rewritten;
                    }
                }
            }
            Value::Object(members) => pending.extend(members.values_mut()),
            Value::Array(items) => pending.extend(items.iter_mut()),
            _ => {}
        }
    }
}
