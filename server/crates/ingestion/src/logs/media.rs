//! A log record's media: the bytes values large enough to be an attachment, stored once per project by content hash.
//!
//! The extraction point is the record's values themselves: its bytes values, and its string values under a member
//! the vocabulary declares may hold media bytes (`may_hold_media_bytes`, as span content reads them) - a producer
//! that writes an image as base64 text under `inlineData.data`. The derived body renders bytes as hex, which no
//! base64 extraction can see, so the bytes are cut out before anything is derived from them, and every derived
//! column holds the reference instead.
//!
//! The reference has two spellings, one per form the record is stored in:
//!
//! - in the record the derived columns are built from, the bytes value becomes the string `#!B64!#<mime>::<hash>` -
//!   a reference string, as span content holds;
//! - in the raw record (`raw_log`, the OTLP/JSON of the record), the value keeps its member and holds the
//!   reference: `"bytesValue": "#!B64!#..."` is not base64, so it cannot be mistaken for bytes, and a media
//!   `stringValue` holds the reference as SideSeat's content does everywhere. Splicing the base64 back from the
//!   file store restores the record exactly. A producer string spelled exactly as a SideSeat media reference reads
//!   as one, as it does in span content: the one convention the product's every reader shares.
//!
//! The content address is BLAKE3 of the base64 text, the file store's and the raw store's own, so an object a span
//! or a raw record already stored is the same object. A record without a trace keeps its bytes inline: a file is
//! owned by `(project, trace, hash)`, and such a record has no trace to own it under. The planned extension is the
//! log's own identity in the trace position; no captured record needs it yet.

use base64::prelude::*;
use opentelemetry_proto::tonic::common::v1::{AnyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use serde_json::Value as JsonValue;

use crate::media::{PendingFileWrite, to_pending_files};
use crate::traces::extract::files::{extract_media_string, is_extractable_key, is_protected_key};

/// The OTLP/JSON member holding a bytes value: the published encoding of `AnyValue.bytes_value`.
const BYTES_VALUE: &str = "bytesValue";
/// The OTLP/JSON member naming a key-value pair's key.
const KEY: &str = "key";
/// The OTLP/JSON member holding a string value.
const STRING_VALUE: &str = "stringValue";

/// A record's media, cut out of both its forms.
pub(crate) struct RecordMedia {
    /// The record the derived columns are built from, its media bytes values replaced by reference strings.
    pub derived: LogRecord,
    /// The files to store, owned by the record's trace.
    pub files: Vec<PendingFileWrite>,
    /// The references that replaced them, one per file.
    pub references: Vec<String>,
}

/// Cut a record's media out of its raw form (`raw`, the record's OTLP/JSON, rewritten in place) and build the
/// record its derived columns are read from. `None` where the record holds no media, or has no trace to own it.
pub(crate) fn extract_record_media(
    record: &LogRecord,
    raw: &mut JsonValue,
    project_id: &str,
    trace_id: Option<&str>,
) -> Option<RecordMedia> {
    let trace_id = trace_id?;
    let mut extracted = Vec::new();
    // Each extracted value's text - a bytes value's base64, a string value's own - to its reference, which is how
    // the derived record's walk finds the same values.
    let mut references = std::collections::HashMap::new();
    replace_media_values(raw, &mut |text| {
        if let Some(uri) = references.get(text) {
            return Some(String::clone(uri));
        }
        // Only text the raw form can give back exactly: canonical standard base64, which encoding the stored bytes
        // reproduces - not a data URL, line-wrapped text or another alphabet, whose bytes are the same but whose
        // spelling a restore could not recover.
        if !reproduces_from_its_bytes(text) {
            return None;
        }
        let (file, uri) = extract_media_string(text, None)?;
        references.insert(text.to_string(), uri.clone());
        extracted.push(file);
        Some(uri)
    });
    if extracted.is_empty() {
        return None;
    }
    let mut derived = record.clone();
    let mut swap = |key: Option<&str>, value: &mut AnyValue| {
        let text = match &value.value {
            Some(any_value::Value::BytesValue(bytes)) => BASE64_STANDARD.encode(bytes),
            Some(any_value::Value::StringValue(text)) if key.is_some_and(holds_media) => {
                text.clone()
            }
            _ => return,
        };
        if let Some(uri) = references.get(&text) {
            value.value = Some(any_value::Value::StringValue(uri.clone()));
        }
    };
    if let Some(body) = derived.body.as_mut() {
        walk_any_value(None, body, &mut swap);
    }
    for attribute in &mut derived.attributes {
        if let Some(value) = attribute.value.as_mut() {
            walk_any_value(Some(attribute.key.as_str()), value, &mut swap);
        }
    }
    let mut references: Vec<String> = references.into_values().collect();
    references.dedup();
    references.sort_unstable();
    let files = to_pending_files(extracted, project_id, trace_id).collect();
    Some(RecordMedia {
        derived,
        files,
        references,
    })
}

/// Escape every producer string of a record's OTLP/JSON (`file_uri::escape_producer_text`), before its media is
/// replaced: the raw form then holds a reference only where SideSeat wrote one, so a producer string spelled like a
/// reference - or like an escaped one - restores as the text it was, never as an image. OTLP/JSON's object keys are
/// its own field names; a producer's keys are string values (`"key": ...`) and are escaped with the rest.
pub(crate) fn escape_raw_record(raw: &mut JsonValue) {
    // Iterative and unbounded: a record nested past any depth limit must still have every producer string escaped,
    // or a string left as written would read as a reference SideSeat wrote.
    let mut pending = vec![raw];
    while let Some(value) = pending.pop() {
        match value {
            JsonValue::String(text) => {
                if let std::borrow::Cow::Owned(escaped) =
                    sideseat_core::utils::file_uri::escape_producer_text(text)
                {
                    *text = escaped;
                }
            }
            JsonValue::Object(members) => pending.extend(members.values_mut()),
            JsonValue::Array(items) => pending.extend(items.iter_mut()),
            _ => {}
        }
    }
}

/// The raw form as the producer wrote it: each SideSeat reference spliced back as the base64 of its object, then every
/// string unescaped. `object` answers a reference's bytes; a reference it cannot answer is left as it is. The proof
/// that the raw form loses nothing; a view shows references, as a span's raw view does, and unescapes only.
#[cfg(test)]
pub(crate) fn restore_raw_record(raw: &mut JsonValue, object: &impl Fn(&str) -> Option<Vec<u8>>) {
    replace_media_values(raw, &mut |text| {
        let parsed = sideseat_core::utils::file_uri::parse_file_uri(text)?;
        Some(BASE64_STANDARD.encode(object(parsed.hash)?))
    });
    let mut pending = vec![raw];
    while let Some(value) = pending.pop() {
        match value {
            JsonValue::String(text) => {
                if let std::borrow::Cow::Owned(original) =
                    sideseat_core::utils::file_uri::unescape_producer_text(text)
                {
                    *text = original;
                }
            }
            JsonValue::Object(members) => pending.extend(members.values_mut()),
            JsonValue::Array(items) => pending.extend(items.iter_mut()),
            _ => {}
        }
    }
}

/// The record the derived columns are read from, rebuilt from its raw form: every producer string unescaped, and
/// each media value SideSeat cut out held as its reference string, as extraction left it - the raw form is the
/// authority every derived column is rebuilt from. `None` where the raw form does not parse as a record.
pub(crate) fn derivation_record(raw: &JsonValue) -> Option<LogRecord> {
    let mut raw = raw.clone();
    let mut pending = vec![&mut raw];
    while let Some(value) = pending.pop() {
        match value {
            JsonValue::String(text) => {
                if let std::borrow::Cow::Owned(original) =
                    sideseat_core::utils::file_uri::unescape_producer_text(text)
                {
                    *text = original;
                }
            }
            JsonValue::Object(members) => {
                // A media bytes value SideSeat replaced reads, in the derived record, as its reference string.
                let replaced = matches!(
                    members.get(BYTES_VALUE),
                    Some(JsonValue::String(text)) if sideseat_core::utils::file_uri::is_file_uri(text)
                );
                if replaced && let Some(reference) = members.remove(BYTES_VALUE) {
                    members.insert(STRING_VALUE.to_string(), reference);
                }
                pending.extend(members.values_mut());
            }
            JsonValue::Array(items) => pending.extend(items.iter_mut()),
            _ => {}
        }
    }
    serde_json::from_value(raw).ok()
}

/// Replace each of these references with its note in every string value of a record's content - values only, so a
/// producer's key keeps its text. The record's derived columns are then rebuilt from it.
pub(crate) fn note_references(record: &mut LogRecord, notes: &[(String, String)]) {
    let mut note = |_: Option<&str>, value: &mut AnyValue| {
        if let Some(any_value::Value::StringValue(text)) = &mut value.value {
            for (uri, replacement) in notes {
                if let Some(rewritten) =
                    sideseat_domain::files::replace_file_reference(text, uri, replacement)
                {
                    *text = rewritten;
                }
            }
        }
    };
    if let Some(body) = record.body.as_mut() {
        walk_any_value(None, body, &mut note);
    }
    for attribute in &mut record.attributes {
        if let Some(value) = attribute.value.as_mut() {
            walk_any_value(Some(attribute.key.as_str()), value, &mut note);
        }
    }
}

/// Whether `text` is the standard base64 of its own bytes, so the raw form restores it exactly from the stored object.
fn reproduces_from_its_bytes(text: &str) -> bool {
    BASE64_STANDARD
        .decode(text)
        .is_ok_and(|bytes| BASE64_STANDARD.encode(bytes) == text)
}

/// Whether a member may hold media bytes as text, and is not prose (the vocabulary's declarations).
fn holds_media(key: &str) -> bool {
    is_extractable_key(key) && !is_protected_key(key)
}

/// Every media value in a record's OTLP/JSON - a `bytesValue`, and a `stringValue` under a member that may hold media
/// bytes - replaced where `extract` returns its reference. Iterative and unbounded, as `walk_any_value` is: the two
/// walks must cut out the same leaves at every depth, or the derived record would differ from the one its raw form
/// rebuilds.
fn replace_media_values(value: &mut JsonValue, extract: &mut impl FnMut(&str) -> Option<String>) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            JsonValue::Object(members) => {
                // A key-value pair whose key may hold media, holding a string: `{"key": K, "value": {"stringValue": S}}`.
                let media_key = members
                    .get(KEY)
                    .and_then(JsonValue::as_str)
                    .is_some_and(holds_media);
                for (key, member) in members.iter_mut() {
                    if key == BYTES_VALUE
                        && let JsonValue::String(text) = member
                    {
                        if let Some(uri) = extract(text) {
                            *text = uri;
                        }
                        continue;
                    }
                    // The pair's other member is its value: an OTLP/JSON key-value pair holds exactly the two.
                    if media_key
                        && key != KEY
                        && let Some(JsonValue::String(text)) = member.get_mut(STRING_VALUE)
                    {
                        if let Some(uri) = extract(text) {
                            *text = uri;
                        }
                        continue;
                    }
                    pending.push(member);
                }
            }
            JsonValue::Array(items) => pending.extend(items.iter_mut()),
            _ => {}
        }
    }
}

/// Visit every leaf value with the key that holds it, where a key-value list names one directly. An array's elements
/// are held by no key: the raw walk (`replace_media_values`) reads a member's media only from its own value, and the
/// two walks must cut out the same leaves, or the derived record would differ from the one its raw form rebuilds.
/// Iterative and unbounded for the same reason.
fn walk_any_value(
    key: Option<&str>,
    value: &mut AnyValue,
    visit: &mut impl FnMut(Option<&str>, &mut AnyValue),
) {
    let mut pending = vec![(key, value)];
    while let Some((key, value)) = pending.pop() {
        if !matches!(
            value.value,
            Some(any_value::Value::ArrayValue(_) | any_value::Value::KvlistValue(_))
        ) {
            visit(key, value);
            continue;
        }
        match value.value.as_mut() {
            Some(any_value::Value::ArrayValue(array)) => {
                pending.extend(array.values.iter_mut().map(|item| (None, item)));
            }
            Some(any_value::Value::KvlistValue(list)) => {
                for member in &mut list.values {
                    if let Some(inner) = member.value.as_mut() {
                        pending.push((Some(member.key.as_str()), inner));
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{KeyValue, KeyValueList};

    /// A PNG's bytes: the magic, then padding to the extraction's minimum size.
    fn png() -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.resize(4096, 7);
        bytes
    }

    fn record(bytes: Vec<u8>) -> LogRecord {
        LogRecord {
            body: Some(AnyValue {
                value: Some(any_value::Value::KvlistValue(KeyValueList {
                    values: vec![KeyValue {
                        key: "inline_data".to_string(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::BytesValue(bytes)),
                        }),
                    }],
                })),
            }),
            ..Default::default()
        }
    }

    /// Both forms hold one reference to one object: the raw record keeps the bytes member, the derived record a
    /// reference string, and the address is BLAKE3 of the base64 text.
    #[test]
    fn a_media_bytes_value_becomes_one_reference_in_both_forms() {
        let record = record(png());
        let mut raw = serde_json::to_value(&record).expect("a record serialises");
        let media = extract_record_media(&record, &mut raw, "p", Some("t")).expect("media");
        assert_eq!(media.files.len(), 1);
        let hash = &media.files[0].hash;
        assert_eq!(
            hash,
            &crate::traces::extract::files::blake3_base64(BASE64_STANDARD.encode(png()).as_bytes())
        );
        let raw_text = raw.to_string();
        assert!(
            raw_text.contains("\"bytesValue\":\"#!B64!#image/png::"),
            "{raw_text}"
        );
        let Some(any_value::Value::KvlistValue(list)) =
            media.derived.body.as_ref().and_then(|b| b.value.clone())
        else {
            panic!("a kvlist body");
        };
        assert!(matches!(
            list.values[0].value.as_ref().and_then(|v| v.value.clone()),
            Some(any_value::Value::StringValue(uri)) if uri.starts_with("#!B64!#image/png::") && uri.ends_with(hash.as_str())
        ));
    }

    /// A record without a trace keeps its bytes inline, and bytes no media type names - an opaque signature - are
    /// not media.
    #[test]
    fn a_traceless_record_and_opaque_bytes_stay_inline() {
        let with_media = record(png());
        let mut raw = serde_json::to_value(&with_media).expect("a record serialises");
        assert!(extract_record_media(&with_media, &mut raw, "p", None).is_none());
        let opaque = record(vec![0x5a; 4096]);
        let mut raw = serde_json::to_value(&opaque).expect("a record serialises");
        let before = raw.clone();
        assert!(extract_record_media(&opaque, &mut raw, "p", Some("t")).is_none());
        assert_eq!(raw, before);
    }

    /// The raw form loses nothing: splicing each referenced object's base64 back into `bytesValue` gives the
    /// record as received, field for field.
    #[test]
    fn the_raw_form_restores_the_record_from_its_objects() {
        let original = record(png());
        let mut raw = serde_json::to_value(&original).expect("a record serialises");
        let media = extract_record_media(&original, &mut raw, "p", Some("t")).expect("media");
        let objects: std::collections::HashMap<String, Vec<u8>> = media
            .files
            .iter()
            .map(|file| {
                let bytes = BASE64_STANDARD.decode(&file.data).expect("base64");
                (file.hash.clone(), bytes)
            })
            .collect();
        restore_raw_record(&mut raw, &|hash| objects.get(hash).cloned());
        let restored: LogRecord = serde_json::from_value(raw).expect("the record parses");
        assert_eq!(restored, original);
    }

    /// An image written as base64 text under a member that may hold media is media too; one written in a spelling
    /// the raw form could not restore exactly - line-wrapped - stays inline.
    #[test]
    fn a_base64_string_under_a_media_member_is_media_unless_its_spelling_could_not_be_restored() {
        let text = BASE64_STANDARD.encode(png());
        let string_record = |text: String| LogRecord {
            body: Some(AnyValue {
                value: Some(any_value::Value::KvlistValue(KeyValueList {
                    values: vec![KeyValue {
                        key: "data".to_string(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(text)),
                        }),
                    }],
                })),
            }),
            ..Default::default()
        };
        let original = string_record(text.clone());
        let mut raw = serde_json::to_value(&original).expect("a record serialises");
        let media = extract_record_media(&original, &mut raw, "p", Some("t")).expect("media");
        assert_eq!(media.files.len(), 1);
        assert!(
            raw.to_string()
                .contains("\"stringValue\":\"#!B64!#image/png::")
        );
        let wrapped = string_record(format!("{}\n{}", &text[..76], &text[76..]));
        let mut raw = serde_json::to_value(&wrapped).expect("a record serialises");
        assert!(extract_record_media(&wrapped, &mut raw, "p", Some("t")).is_none());
    }

    /// The derived record extraction builds is the one its raw form rebuilds, so a re-derivation changes nothing -
    /// here where the same image is a bytes value and also base64 text in an array under a media member, which
    /// neither walk reads as media, so it stays inline in both forms.
    #[test]
    fn the_derived_record_is_the_one_its_raw_form_rebuilds() {
        let text = BASE64_STANDARD.encode(png());
        let mut original = record(png());
        original.attributes.push(KeyValue {
            key: "data".to_string(),
            value: Some(AnyValue {
                value: Some(any_value::Value::ArrayValue(
                    opentelemetry_proto::tonic::common::v1::ArrayValue {
                        values: vec![AnyValue {
                            value: Some(any_value::Value::StringValue(text.clone())),
                        }],
                    },
                )),
            }),
        });
        let mut raw = serde_json::to_value(&original).expect("a record serialises");
        escape_raw_record(&mut raw);
        let media = extract_record_media(&original, &mut raw, "p", Some("t")).expect("media");
        assert_eq!(derivation_record(&raw).as_ref(), Some(&media.derived));
        assert_eq!(media.derived.attributes, original.attributes);
    }

    /// The two walks agree at any depth: the same image, once shallow and once nested in arrays deeper than any
    /// limit, is cut out of both forms alike.
    #[test]
    fn the_walks_agree_whatever_the_nesting() {
        let mut deep = AnyValue {
            value: Some(any_value::Value::BytesValue(png())),
        };
        for _ in 0..64 {
            deep = AnyValue {
                value: Some(any_value::Value::ArrayValue(
                    opentelemetry_proto::tonic::common::v1::ArrayValue { values: vec![deep] },
                )),
            };
        }
        let mut original = record(png());
        original.attributes.push(KeyValue {
            key: "nested".to_string(),
            value: Some(deep),
        });
        let mut raw = serde_json::to_value(&original).expect("a record serialises");
        escape_raw_record(&mut raw);
        let media = extract_record_media(&original, &mut raw, "p", Some("t")).expect("media");
        assert_eq!(media.files.len(), 1);
        assert_eq!(
            raw.to_string().matches("\"bytesValue\":\"#!B64!#").count(),
            2
        );
        assert_eq!(derivation_record(&raw).as_ref(), Some(&media.derived));
    }

    /// A producer string spelled exactly like a reference, and one spelled like an escaped reference, restore as the
    /// text they were: the raw form holds a reference only where SideSeat wrote one.
    #[test]
    fn a_producer_string_spelled_like_a_reference_restores_as_text() {
        let reference = format!("#!B64!#image/png::{}", "a".repeat(64));
        let mut original = record(png());
        original.attributes = vec![
            KeyValue {
                key: "note".to_string(),
                value: Some(AnyValue {
                    value: Some(any_value::Value::StringValue(reference.clone())),
                }),
            },
            KeyValue {
                key: "escaped".to_string(),
                value: Some(AnyValue {
                    value: Some(any_value::Value::StringValue(format!(
                        "see #!!{}",
                        &reference[2..]
                    ))),
                }),
            },
        ];
        let mut raw = serde_json::to_value(&original).expect("a record serialises");
        escape_raw_record(&mut raw);
        let media = extract_record_media(&original, &mut raw, "p", Some("t")).expect("media");
        let mut uris = Vec::new();
        sideseat_domain::files::collect_file_references_in_str(&raw.to_string(), &mut uris);
        assert_eq!(
            uris.len(),
            1,
            "only SideSeat's reference reads as one: {uris:?}"
        );
        let objects: std::collections::HashMap<String, Vec<u8>> = media
            .files
            .iter()
            .map(|file| {
                (
                    file.hash.clone(),
                    BASE64_STANDARD.decode(&file.data).expect("base64"),
                )
            })
            .collect();
        restore_raw_record(&mut raw, &|hash| objects.get(hash).cloned());
        let restored: LogRecord = serde_json::from_value(raw).expect("the record parses");
        assert_eq!(restored, original);
    }

    /// Escaping reaches every producer string whatever the nesting: a record nested past any depth limit still holds
    /// no reference SideSeat did not write.
    #[test]
    fn a_deeply_nested_producer_string_is_escaped() {
        let mut raw = serde_json::json!("#!B64!#image/png::aa");
        for _ in 0..500 {
            raw = serde_json::json!({"arrayValue": {"values": [raw]}});
        }
        escape_raw_record(&mut raw);
        assert!(!raw.to_string().contains("#!B64!#"));
    }
}
