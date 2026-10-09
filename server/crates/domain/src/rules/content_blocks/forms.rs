//! The forms that read more than one member's worth of a block: a text's citations, a tool the provider ran
//! itself written as one item, and media, whose kind, source and type are each derived.

use serde_json::{Value as JsonValue, json};

use super::super::schema::{
    CitationsSpec, MediaBlock, MediaSource, MissingMediaType, ProviderRunSpec, ValueSource,
};
use super::{call_id, member, predicates_hold, query, result_blocks};

/// The citations a text states, each read by the first case whose condition holds. A citation no case reads, or
/// one that names no source, is kept as `unknown` with the provider's item, so a shape nothing reads yet is seen.
pub(super) fn citations(block: &JsonValue, spec: &CitationsSpec) -> Vec<JsonValue> {
    let Some(list) = spec
        .from
        .iter()
        .find_map(|path| query(block, path).into_iter().next())
        .and_then(JsonValue::as_array)
    else {
        return Vec::new();
    };
    let text = |item: &JsonValue, sources: &[ValueSource]| {
        member(item, sources, false).and_then(|v| v.as_str().map(str::to_string))
    };
    let offset = |item: &JsonValue, sources: &[ValueSource]| {
        member(item, sources, false).and_then(|v| v.as_u64())
    };
    list.iter()
        .map(|item| {
            let read = spec
                .cases
                .iter()
                .find(|case| predicates_hold(item, &case.require))
                .and_then(|case| Some((case, text(item, &case.source)?)));
            let Some((case, source)) = read else {
                return json!({"kind": "unknown", "raw": item});
            };
            let mut citation = serde_json::Map::new();
            citation.insert("kind".to_string(), json!(<&str>::from(case.kind)));
            citation.insert("source".to_string(), json!(source));
            for (key, value) in [
                ("title", text(item, &case.title).map(JsonValue::from)),
                (
                    "text_start",
                    offset(item, &case.text_start).map(JsonValue::from),
                ),
                (
                    "text_end",
                    offset(item, &case.text_end).map(JsonValue::from),
                ),
                (
                    "cited_text",
                    text(item, &case.cited_text).map(JsonValue::from),
                ),
            ] {
                if let Some(value) = value {
                    citation.insert(key.to_string(), value);
                }
            }
            JsonValue::Object(citation)
        })
        .collect()
}

/// A provider-run item as its two blocks: the call, and what it produced where the item holds a result yet.
/// Both carry `provider_executed`, so the pair is read as the response's own output.
pub(super) fn provider_run(block: &JsonValue, spec: &ProviderRunSpec) -> Option<Vec<JsonValue>> {
    let name = member(block, &spec.name, false)?;
    let name = name.as_str()?;
    let input = member(block, &spec.input, true)
        .map(std::borrow::Cow::into_owned)
        .unwrap_or_else(|| json!({}));
    let id = call_id(block, &spec.id, name, &input);
    let mut blocks = vec![json!({
        "type": "tool_use",
        "id": id,
        "name": name,
        "input": input,
        "provider_executed": true,
    })];
    if let Some(result) = member(block, &spec.result, false) {
        blocks.push(json!({
            "type": "tool_result",
            "tool_use_id": id,
            "content": result_blocks(Some(result.into_owned())),
            "provider_executed": true,
        }));
    }
    Some(blocks)
}

/// A media block, or nothing where the case cannot say what the bytes are.
pub(super) fn media_block(block: &JsonValue, spec: &MediaBlock) -> Option<JsonValue> {
    let data = member(block, &spec.data, false)?;
    let data = data.as_str()?;
    let declared: Option<String> = member(block, &spec.media_type, false)
        .and_then(|v| v.as_str().map(str::to_string))
        .or_else(|| spec.media_type_default.clone());
    let name = member(block, &spec.name, false).and_then(|v| v.as_str().map(str::to_string));
    let detail = member(block, &spec.detail, false).and_then(|v| v.as_str().map(str::to_string));
    let (source, referenced, payload): (&str, Option<&str>, &str) = match &spec.source {
        // Both derived, because both are facts about the bytes rather than about the producer: the kind
        // comes from the media type, and whether this is a reference or the content itself from the value.
        MediaSource::Decoded => {
            let (source, referenced) = crate::sideml::content::decode_media_source(data);
            // A data URL's header has been read for the media type; the block holds the payload alone.
            let payload = match data.split_once(',') {
                Some((header, payload)) if source == "base64" && header.starts_with("data:") => {
                    payload
                }
                _ => data,
            };
            (source, referenced, payload)
        }
        // The member's meaning is the format's: a stored reference is still recognised, because ingestion
        // replaces inline bytes with one, but nothing else about the value is second-guessed.
        MediaSource::ReferenceOr(otherwise) => {
            let source = if sideseat_core::utils::file_uri::is_file_uri(data) {
                "file"
            } else {
                otherwise.as_str()
            };
            (source, None, data)
        }
        MediaSource::Literal(source) => (source.as_str(), None, data),
    };
    // **One authority.** A stored reference carries the media type the bytes were stored under, and a
    // declared one beside it could disagree - `media_type: image/png` against `#!B64!#application/pdf::HASH`
    // produced an image block whose bytes a reader fetches as a PDF. The reference wins, because it is the
    // *stored* fact and what a fetch will return; the disagreement is reported rather than refused, since
    // refusing drops content over metadata.
    let media_type: Option<String> = match referenced {
        Some(stored) => {
            if let Some(declared) = declared.as_deref().filter(|declared| *declared != stored) {
                tracing::debug!(
                    target: "sideseat::rules",
                    declared,
                    stored,
                    "a media block's declared media type disagrees with its stored reference; the stored one \
                     is what a reader will fetch"
                );
            }
            Some(stored.to_string())
        }
        // The conventions make a blob's MIME type optional; without one, the bytes say what they are - asked
        // only where the value's shape is the authority, and only when a missing type would decline.
        None => declared.or_else(|| {
            (spec.source == MediaSource::Decoded
                && spec.missing_media_type == MissingMediaType::Decline)
                .then(|| sideseat_core::utils::mime::detect_mime_type_from_base64(data.as_bytes()))
                .flatten()
                .map(str::to_string)
        }),
    };
    if media_type.is_none() && spec.missing_media_type == MissingMediaType::Decline {
        return None;
    }
    let kind: &str = match spec.kind {
        Some(kind) => kind.into(),
        None => crate::sideml::content::mime_to_content_type(media_type.as_deref().unwrap_or("")),
    };
    let mut result = serde_json::Map::new();
    result.insert("type".to_string(), json!(kind));
    if !(media_type.is_none() && spec.missing_media_type == MissingMediaType::Omit) {
        result.insert("media_type".to_string(), json!(media_type));
    }
    result.insert("source".to_string(), json!(source));
    result.insert("data".to_string(), json!(payload));
    if let Some(name) = name {
        result.insert("name".to_string(), json!(name));
    }
    if let Some(detail) = detail {
        result.insert("detail".to_string(), json!(detail));
    }
    Some(JsonValue::Object(result))
}
