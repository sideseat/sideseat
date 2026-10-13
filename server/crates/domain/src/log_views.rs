//! A log record's views - its body as JSON and as text, and its attributes as typed JSON - rebuilt from the stored
//! raw record.
//!
//! The raw record (`raw_log`, the record's OTLP/JSON with every producer string escaped and each media value
//! SideSeat stored replaced by its reference) is the authority, and the views are functions of it and of the
//! references ingest found unbacked in the project's store, which they show as a note. So no view is stored: ingest
//! checks that the raw record rebuilds what it derived from the record itself, byte for byte ([`rebuilds`]), and
//! stores the views only for a record whose raw form does not (`NormalizedLog::stored_views`). A read rebuilds the
//! rest here ([`resolve_views`]).
//!
//! The rendering mirrors the ingest one over the decoded record - a string as itself, an integer and a finite double
//! as numbers, bytes as hex in the body, an array and a key-value list in their order, a repeated key's last value
//! in its first place - and any divergence costs only bytes: ingest compares the two and stores the views where
//! they differ. An attribute holding bytes or a double JSON cannot hold is one the typed rendering tags; this one
//! does not rebuild it, so such a record keeps its views stored.

use std::collections::HashMap;

use base64::prelude::*;
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use sideseat_core::utils::file_uri::{is_file_uri, unescape_producer_text};

use sideseat_ports::types::{LogRow, LogSearchSource};

use crate::files::{missing_content_note, replace_file_reference};

/// A record's views, each as its stored column holds it: the body's JSON text, absent for a record without a body;
/// its text rendering, absent where it is empty; and the attributes' typed JSON text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogViews {
    pub body: Option<String>,
    pub body_text: Option<String>,
    pub attributes: Option<String>,
}

/// Whether a raw record, read back as stored, rebuilds exactly the views ingest derived from the record itself.
pub fn rebuilds(raw_log: &str, noted: &[String], derived: &LogViews) -> bool {
    rebuild_views(raw_log, noted).as_ref() == Some(derived)
}

/// The views a read answers, from the columns ingest stored (`stored`), the raw record and the references it
/// noted: the stored views where there are any - an empty body text standing for a record stored with none
/// (`NormalizedLog::stored_views`) - and the rebuilt ones otherwise.
pub fn resolve_views(stored: LogViews, raw_log: Option<&str>, noted: &[String]) -> LogViews {
    match stored {
        LogViews {
            body: None,
            body_text: None,
            attributes: None,
        } => raw_log
            .and_then(|raw| rebuild_views(raw, noted))
            .unwrap_or_default(),
        LogViews {
            body: None,
            body_text: Some(text),
            attributes: None,
        } if text.is_empty() => LogViews::default(),
        stored => stored,
    }
}

/// A stored log row with its views answered: as stored, or rebuilt from its raw record.
pub fn resolve_row(mut row: LogRow) -> LogRow {
    let views = resolve_views(
        LogViews {
            body: row.body.take(),
            body_text: row.body_text.take(),
            attributes: row.attributes.take(),
        },
        row.raw_log.as_deref(),
        &row.noted_references,
    );
    row.body = views.body;
    row.body_text = views.body_text;
    row.attributes = views.attributes;
    row
}

/// A log's search source with its views answered, as [`resolve_row`] answers a row's.
pub fn resolve_source(source: &LogSearchSource) -> LogViews {
    resolve_views(
        LogViews {
            body: source.body.clone(),
            body_text: source.body_text.clone(),
            attributes: source.attributes.clone(),
        },
        source.raw_log.as_deref(),
        &source.noted_references,
    )
}

/// The views a raw record rebuilds, with each of the `noted` references shown as its note. `None` where the raw
/// record is not one this rendering reads - not JSON, or a value it cannot decode or render.
pub fn rebuild_views(raw_log: &str, noted: &[String]) -> Option<LogViews> {
    let record = object(serde_json::from_str::<&RawValue>(raw_log).ok()?)?;
    let notes: Vec<(String, String)> = noted
        .iter()
        .map(|uri| (uri.clone(), missing_content_note(uri)))
        .collect();
    let (body, body_text) = match present(record.get("body").copied()) {
        None => (None, None),
        Some(body) => {
            let value = decode(body, &notes)?;
            let mut json = to_json(&value);
            rewrite_values(&mut json, &notes);
            let text = to_text(&value);
            (
                (!json.is_null()).then(|| json.to_string()),
                (!text.is_empty()).then_some(text),
            )
        }
    };
    let mut attributes = Map::new();
    for (key, value) in decode_pairs(record.get("attributes").copied(), &notes)? {
        attributes.insert(key, value.as_ref().map_or(Some(Value::Null), to_typed)?);
    }
    let mut attributes = Value::Object(attributes);
    rewrite_values(&mut attributes, &notes);
    Some(LogViews {
        body,
        body_text,
        attributes: Some(attributes.to_string()),
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

/// An OTLP `AnyValue`, decoded from its OTLP/JSON with producer strings unescaped and notes applied.
enum Any {
    Empty,
    String(String),
    Bool(bool),
    Int(i64),
    Double(f64),
    Bytes(Vec<u8>),
    Array(Vec<Any>),
    /// In the producer's order; a member without a value is `None`, which the body's renderings leave out and the
    /// attributes' renders as `null`, as ingest does.
    Kvlist(Vec<(String, Option<Any>)>),
}

fn decode(value: &RawValue, notes: &[(String, String)]) -> Option<Any> {
    let members = object(value)?;
    if let Some(text) = members.get("stringValue") {
        return Some(Any::String(noted_text(&string(text)?, notes)));
    }
    if let Some(flag) = members.get("boolValue") {
        return serde_json::from_str(flag.get()).ok().map(Any::Bool);
    }
    if let Some(number) = members.get("intValue") {
        let text = number.get();
        return Some(Any::Int(if text.starts_with('"') {
            string(number)?.parse().ok()?
        } else {
            text.parse().ok()?
        }));
    }
    if let Some(number) = members.get("doubleValue") {
        // From its digits, as Rust's correctly rounded parse reads them: the shortest text a double renders to
        // parses back to the same double.
        return number.get().parse().ok().map(Any::Double);
    }
    if let Some(bytes) = members.get("bytesValue") {
        let text = string(bytes)?;
        // A media value SideSeat stored reads as its reference, a string, as the decoded record holds it.
        if is_file_uri(&text) {
            return Some(Any::String(noted_text(&text, notes)));
        }
        return Some(Any::Bytes(BASE64_STANDARD.decode(text).ok()?));
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
            .map(Any::Array);
    }
    if let Some(kvlist) = members.get("kvlistValue") {
        return decode_pairs(object(kvlist)?.get("values").copied(), notes).map(Any::Kvlist);
    }
    members.is_empty().then_some(Any::Empty)
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
) -> Option<Vec<(String, Option<Any>)>> {
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
                .filter_map(|(key, value)| Some((key.clone(), to_json(value.as_ref()?))))
                .collect::<Map<String, Value>>(),
        ),
    }
}

/// An attribute value as ingest types it (`attrs_to_typed_json`): JSON's own kinds as themselves, an absent value
/// as `null`. `None` for bytes and a double JSON cannot hold, which ingest tags: a record holding either keeps its
/// views stored rather than this rendering learn the tag.
fn to_typed(value: &Any) -> Option<Value> {
    Some(match value {
        Any::Empty => Value::Null,
        Any::String(text) => Value::String(text.clone()),
        Any::Bool(flag) => Value::Bool(*flag),
        Any::Int(number) => Value::from(*number),
        Any::Double(number) => Value::Number(serde_json::Number::from_f64(*number)?),
        Any::Bytes(_) => return None,
        Any::Array(items) => Value::Array(items.iter().map(to_typed).collect::<Option<_>>()?),
        Any::Kvlist(pairs) => {
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
