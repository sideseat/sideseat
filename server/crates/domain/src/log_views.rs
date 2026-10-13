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

use serde_json::{Map, Value};
use sideseat_ports::types::{LogRow, LogSearchSource};

use crate::log_record::{
    LogRecordContent, from_raw, notes_for, rewrite_values, to_json, to_text, to_typed,
};

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
    let notes = notes_for(noted);
    views(&from_raw(raw_log, &notes)?, &notes)
}

/// A decoded record's views, its notes applied after rendering as ingest applies them. `None` where an attribute
/// holds a value the typed rendering tags.
fn views(record: &LogRecordContent, notes: &[(String, String)]) -> Option<LogViews> {
    let (body, body_text) = match &record.body {
        None => (None, None),
        Some(value) => {
            let mut json = to_json(value);
            rewrite_values(&mut json, notes);
            let text = to_text(value);
            (
                (!json.is_null()).then(|| json.to_string()),
                (!text.is_empty()).then_some(text),
            )
        }
    };
    let mut attributes = Map::new();
    for (key, value) in &record.attributes {
        attributes.insert(
            key.clone(),
            value.as_ref().map_or(Some(Value::Null), to_typed)?,
        );
    }
    let mut attributes = Value::Object(attributes);
    rewrite_values(&mut attributes, notes);
    Some(LogViews {
        body,
        body_text,
        attributes: Some(attributes.to_string()),
    })
}
