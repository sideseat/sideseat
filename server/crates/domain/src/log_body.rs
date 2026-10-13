//! A log record's body views - the body as JSON and as text - rebuilt from the stored raw record.
//!
//! The raw record (`raw_log`, the record's OTLP/JSON with every producer string escaped and each media value
//! SideSeat stored replaced by its reference) is the authority, and the two views are functions of it and of the
//! references ingest found unbacked in the project's store, which they show as a note. So neither view is stored:
//! ingest checks that the raw record rebuilds what it derived from the record itself, byte for byte
//! ([`rebuilds`]), and stores the views only for a record whose raw form does not
//! (`NormalizedLog::stored_body_views`). A read rebuilds the rest here ([`resolve_views`]).
//!
//! The rendering mirrors the ingest one over the decoded record - a string as itself, an integer and a finite double
//! as numbers, bytes as hex, an array and a key-value list in their order, a repeated key's last value in its first
//! place - and any divergence costs only bytes: ingest compares the two and stores the views where they differ.

use std::collections::HashMap;

use base64::prelude::*;
use serde_json::{Map, Value};
use sideseat_core::utils::file_uri::{is_file_uri, unescape_producer_text};

use sideseat_ports::types::{LogRow, LogSearchSource};

use crate::files::{missing_content_note, replace_file_reference};

/// A record's two body views, each as its stored column holds it: the body's JSON text, absent for a record without
/// a body, and its text rendering, absent where it is empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BodyViews {
    pub body: Option<String>,
    pub body_text: Option<String>,
}

/// Whether a raw record, read back as stored, rebuilds exactly the views ingest derived from the record itself.
pub fn rebuilds(raw_log: &str, noted: &[String], derived: &BodyViews) -> bool {
    rebuild_views(raw_log, noted).as_ref() == Some(derived)
}

/// The views a read answers, from the columns ingest stored (`stored`), the raw record and the references it
/// noted: the stored views where there are any - an empty body text standing for a record stored with neither
/// (`NormalizedLog::stored_body_views`) - and the rebuilt ones otherwise.
pub fn resolve_views(stored: BodyViews, raw_log: Option<&str>, noted: &[String]) -> BodyViews {
    match stored {
        BodyViews {
            body: None,
            body_text: None,
        } => raw_log
            .and_then(|raw| rebuild_views(raw, noted))
            .unwrap_or_default(),
        BodyViews {
            body: None,
            body_text: Some(text),
        } if text.is_empty() => BodyViews::default(),
        stored => stored,
    }
}

/// A stored log row with its body views answered: as stored, or rebuilt from its raw record.
pub fn resolve_row(mut row: LogRow) -> LogRow {
    let views = resolve_views(
        BodyViews {
            body: row.body.take(),
            body_text: row.body_text.take(),
        },
        row.raw_log.as_deref(),
        &row.noted_references,
    );
    row.body = views.body;
    row.body_text = views.body_text;
    row
}

/// A log's search source with its body views answered, as [`resolve_row`] answers a row's.
pub fn resolve_source(source: &LogSearchSource) -> BodyViews {
    resolve_views(
        BodyViews {
            body: source.body.clone(),
            body_text: source.body_text.clone(),
        },
        source.raw_log.as_deref(),
        &source.noted_references,
    )
}

/// The views a raw record rebuilds, with each of the `noted` references shown as its note. `None` where the raw
/// record is not one this rendering reads - not JSON, or a body value it cannot decode.
pub fn rebuild_views(raw_log: &str, noted: &[String]) -> Option<BodyViews> {
    let record: Value = serde_json::from_str(raw_log).ok()?;
    let Some(body) = record.get("body").filter(|body| !body.is_null()) else {
        return Some(BodyViews::default());
    };
    let notes: Vec<(String, String)> = noted
        .iter()
        .map(|uri| (uri.clone(), missing_content_note(uri)))
        .collect();
    let value = decode(body, &notes)?;
    let mut json = to_json(&value);
    rewrite_values(&mut json, &notes);
    let text = to_text(&value);
    Some(BodyViews {
        body: (!json.is_null()).then(|| json.to_string()),
        body_text: (!text.is_empty()).then_some(text),
    })
}

/// An OTLP `AnyValue`, decoded from its OTLP/JSON with producer strings unescaped and notes applied.
enum Any {
    Empty,
    String(String),
    Bool(bool),
    Int(i64),
    Double(f64),
    Bytes(Vec<u8>),
    Array(Vec<Any>),
    /// In the producer's order; a member without a value is kept out.
    Kvlist(Vec<(String, Any)>),
}

fn decode(value: &Value, notes: &[(String, String)]) -> Option<Any> {
    let members = value.as_object()?;
    if let Some(text) = members.get("stringValue") {
        let mut text = unescape(text.as_str()?);
        for (uri, note) in notes {
            if let Some(rewritten) = replace_file_reference(&text, uri, note) {
                text = rewritten;
            }
        }
        return Some(Any::String(text));
    }
    if let Some(flag) = members.get("boolValue") {
        return Some(Any::Bool(flag.as_bool()?));
    }
    if let Some(number) = members.get("intValue") {
        return Some(Any::Int(match number {
            Value::String(text) => text.parse().ok()?,
            other => other.as_i64()?,
        }));
    }
    if let Some(number) = members.get("doubleValue") {
        return Some(Any::Double(number.as_f64()?));
    }
    if let Some(bytes) = members.get("bytesValue") {
        let text = bytes.as_str()?;
        // A media value SideSeat stored reads as its reference, a string, as the decoded record holds it.
        if is_file_uri(text) {
            return decode(&serde_json::json!({ "stringValue": text }), notes);
        }
        return Some(Any::Bytes(BASE64_STANDARD.decode(text).ok()?));
    }
    if let Some(array) = members.get("arrayValue") {
        let items = list_values(array)?;
        return items
            .iter()
            .map(|item| decode(item, notes))
            .collect::<Option<Vec<_>>>()
            .map(Any::Array);
    }
    if let Some(list) = members.get("kvlistValue") {
        let mut pairs = Vec::new();
        for member in list_values(list)? {
            let member = member.as_object()?;
            let key = unescape(member.get("key").map_or(Some(""), Value::as_str)?);
            match member.get("value") {
                None | Some(Value::Null) => {}
                Some(value) => pairs.push((key, decode(value, notes)?)),
            }
        }
        return Some(Any::Kvlist(pairs));
    }
    members.is_empty().then_some(Any::Empty)
}

/// The `values` of an array or key-value list, absent when the list is empty.
fn list_values(list: &Value) -> Option<&[Value]> {
    match list.as_object()?.get("values") {
        None | Some(Value::Null) => Some(&[]),
        Some(values) => values.as_array().map(Vec::as_slice),
    }
}

fn unescape(text: &str) -> String {
    unescape_producer_text(text).into_owned()
}

fn to_json(value: &Any) -> Value {
    match value {
        Any::Empty => Value::Null,
        Any::String(text) => Value::String(text.clone()),
        Any::Bool(flag) => Value::Bool(*flag),
        Any::Int(number) => Value::from(*number),
        Any::Double(number) => {
            serde_json::Number::from_f64(*number).map_or(Value::Null, Value::Number)
        }
        Any::Bytes(bytes) => Value::String(hex::encode(bytes)),
        Any::Array(items) => Value::Array(items.iter().map(to_json).collect()),
        Any::Kvlist(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(key, value)| (key.clone(), to_json(value)))
                .collect::<Map<String, Value>>(),
        ),
    }
}

fn to_text(value: &Any) -> String {
    match value {
        Any::Empty => String::new(),
        Any::String(text) => text.clone(),
        Any::Bool(flag) => flag.to_string(),
        Any::Int(number) => number.to_string(),
        Any::Double(number) => number.to_string(),
        Any::Bytes(bytes) => hex::encode(bytes),
        Any::Array(items) => {
            let values: Vec<String> = items.iter().map(to_text).collect();
            serde_json::to_string(&values).unwrap_or_default()
        }
        Any::Kvlist(members) => {
            let mut pairs: Vec<(String, String)> = Vec::new();
            let mut position: HashMap<&str, usize> = HashMap::new();
            for (key, value) in members {
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

/// Each note in every string value of the body's JSON, as ingest applies them after deriving it.
fn rewrite_values(value: &mut Value, notes: &[(String, String)]) {
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
