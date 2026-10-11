//! Member selection: the first source that resolves to something the field accepts.

use std::borrow::Cow;

use serde_json::Value as JsonValue;

use super::query;
use crate::rules::schema::{TransformedSource, ValueSource};

/// The first source that resolves to something this member accepts.
///
/// `empty_object_is_absent` is the one non-obvious rule and it is a producer fact: a dialect writes a
/// call's arguments under one member and, in an older version, under another - and the unused one is
/// present as `{}` rather than missing, so "the first that resolves" would always pick the empty one.
pub(super) fn member<'b>(
    block: &'b JsonValue,
    sources: &[ValueSource],
    empty_object_is_absent: bool,
) -> Option<Cow<'b, JsonValue>> {
    usable_member(block, sources, empty_object_is_absent, |_| true)
}

/// [`member`], stepping over a candidate that is there but cannot be read as the field needs.
///
/// This is `mode: "usable"`'s runtime half: the fields typed `FirstOf<_, true>` declare that semantics, and
/// without the check here a first spelling holding the wrong type - `isError: "bad"` beside `is_error: true` -
/// committed the field to a value its reader then dropped.
pub(super) fn usable_member<'b>(
    block: &'b JsonValue,
    sources: &[ValueSource],
    empty_object_is_absent: bool,
    need: impl Fn(&JsonValue) -> bool,
) -> Option<Cow<'b, JsonValue>> {
    sources.iter().find_map(|source| {
        let found = match source {
            ValueSource::Path(path) => query(block, path).into_iter().next().map(Cow::Borrowed),
            ValueSource::Transformed(spec) => transformed(block, spec),
        }?;
        (!(empty_object_is_absent && found.as_object().is_some_and(serde_json::Map::is_empty))
            && need(&found))
        .then_some(found)
    })
}

/// A source's value after its one transform, or nothing where the transform cannot apply.
fn transformed<'b>(block: &'b JsonValue, spec: &TransformedSource) -> Option<Cow<'b, JsonValue>> {
    if let Some(separator) = spec.join() {
        let texts: Vec<&str> = query(block, &spec.path)
            .into_iter()
            .filter_map(JsonValue::as_str)
            .collect();
        return (!texts.is_empty()).then(|| Cow::Owned(JsonValue::String(texts.join(separator))));
    }
    let found = query(block, &spec.path).into_iter().next()?;
    if let Some(mode) = spec.parse() {
        let Some(text) = found.as_str() else {
            return Some(Cow::Borrowed(found));
        };
        return crate::rules::message_rules::parse_value(text, mode).map(Cow::Owned);
    }
    if let Some(prefix) = spec.prepend() {
        return found
            .as_str()
            .map(|text| Cow::Owned(JsonValue::String(format!("{prefix}{text}"))));
    }
    if let Some(table) = spec.map() {
        return found
            .as_str()
            .and_then(|text| table.get(text))
            .map(|mapped| Cow::Owned(mapped.clone()));
    }
    None
}

/// The path a source reads, whatever it then does with the value.
pub(super) fn source_path(source: &ValueSource) -> &crate::rules::schema::JsonPath {
    match source {
        ValueSource::Path(path) => path,
        ValueSource::Transformed(spec) => &spec.path,
    }
}
