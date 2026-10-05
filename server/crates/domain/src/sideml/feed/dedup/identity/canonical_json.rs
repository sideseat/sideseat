//! The canonical form of a JSON value for identity: key order, and which members carry no value.

use std::hash::{Hash, Hasher};

/// Normalize JSON for consistent hashing: sort object keys.
pub(in crate::sideml::feed) fn normalize_json_for_hash(value: &serde_json::Value) -> String {
    normalize_json(value, EmptyMembers::Keep)
}

/// As [`normalize_json_for_hash`], but treating a member with no value as absent.
///
/// For a **structured answer only**. Filling a schema adds the fields the model did not produce,
/// as `null`, `[]`, `{}` or `""`, so an SDK that reports both the model's raw object and its
/// schema-shaped one - Vercel's `generateObject` puts the raw object on the inner span and the
/// normalized one on the outer - emits the same answer twice with different bytes, and both
/// reached the feed as separate assistant messages.
///
/// Deliberately not applied to tool inputs or tool results, where an explicitly empty collection
/// is a different answer from a missing one: a search result of `{"results": []}` says "no
/// matches" and `{}` says nothing, and collapsing those would drop a real message. The
/// distinction only stops mattering once a schema has supplied the empty value itself.
///
/// Only identity is affected: what the API returns is still the block's own content.
#[cfg(test)]
pub(in crate::sideml::feed) fn normalize_structured_json_for_hash(
    value: &serde_json::Value,
) -> String {
    normalize_json(value, EmptyMembers::Drop)
}

/// Whether object members carrying no value take part in identity.
#[derive(Clone, Copy, PartialEq)]
enum EmptyMembers {
    Keep,
    Drop,
    /// Only a `null` member of this object is absent; values below it keep every member.
    NullArguments,
}

impl EmptyMembers {
    fn omits(self, member: &serde_json::Value) -> bool {
        match self {
            Self::Keep => false,
            Self::Drop => is_empty_json_member(member),
            Self::NullArguments => member.is_null(),
        }
    }

    /// The treatment of the values inside a member.
    fn below(self) -> Self {
        match self {
            Self::NullArguments => Self::Keep,
            other => other,
        }
    }
}

fn normalize_json(value: &serde_json::Value, empty: EmptyMembers) -> String {
    use serde_json::Value as JsonValue;
    match value {
        JsonValue::Object(map) => {
            let mut pairs: Vec<_> = map.iter().filter(|(_, v)| !empty.omits(v)).collect();
            pairs.sort_by_key(|(k, _)| *k);
            let sorted: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}:{}", k, normalize_json(v, empty.below())))
                .collect();
            format!("{{{}}}", sorted.join(","))
        }
        JsonValue::Array(arr) => {
            let items: Vec<String> = arr
                .iter()
                .map(|v| normalize_json(v, empty.below()))
                .collect();
            format!("[{}]", items.join(","))
        }
        _ => value.to_string(),
    }
}

/// Feed a JSON value's canonical form to a hasher **without building it**.
///
/// The canonical form used to be materialised as a `String` - nested `format!` and `join` at every
/// level - purely so it could be hashed. For a tool result carrying a base64 image that copies
/// megabytes once per nesting level, and it happens twice per block (once in flatten, again in
/// history marking), which was 620 ms of a 700 ms request on `vercel-ai-js/image-gen`.
///
/// The bytes differ from the old string form, so hash *values* differ; what matters is preserved,
/// which is that equal content hashes equally and unequal content does not. Lengths are hashed
/// alongside the members so that concatenation cannot make two different shapes agree.
pub(in crate::sideml::feed) fn hash_json_into<H: Hasher>(
    value: &serde_json::Value,
    hasher: &mut H,
) {
    hash_json_streaming(value, EmptyMembers::Keep, hasher);
}

/// As [`hash_json_into`], for a tool call's arguments: an argument passed as `null` is the argument
/// left out.
///
/// A framework that reports the call it executed beside the call the model made fills the optional
/// parameters it injects with `None`, so the execution's arguments carry members the model never
/// wrote. Semantic Kernel adds `instructions_override: null` to every function it invokes for an agent,
/// and the one call then appeared twice. Only a top-level `null` is dropped: an empty collection is
/// still an argument (see [`normalize_structured_json_for_hash`]), and so is a `null` nested inside
/// an argument's value.
pub(in crate::sideml::feed) fn hash_tool_input_into<H: Hasher>(
    value: &serde_json::Value,
    hasher: &mut H,
) {
    hash_json_streaming(value, EmptyMembers::NullArguments, hasher);
}

/// As [`hash_json_into`], but treating a member with no value as absent - see
/// [`normalize_structured_json_for_hash`] for why that is right for a structured answer only.
pub(in crate::sideml::feed) fn hash_structured_json_into<H: Hasher>(
    value: &serde_json::Value,
    hasher: &mut H,
) {
    hash_json_streaming(value, EmptyMembers::Drop, hasher);
}

fn hash_json_streaming<H: Hasher>(value: &serde_json::Value, empty: EmptyMembers, hasher: &mut H) {
    use serde_json::Value as JsonValue;
    // A per-kind tag, so a string cannot hash as the object whose rendering it matches.
    match value {
        JsonValue::Object(map) => {
            0u8.hash(hasher);
            let mut keys: Vec<&str> = map
                .iter()
                .filter(|(_, v)| !empty.omits(v))
                .map(|(k, _)| k.as_str())
                .collect();
            keys.sort_unstable();
            keys.len().hash(hasher);
            for key in keys {
                key.hash(hasher);
                if let Some(member) = map.get(key) {
                    hash_json_streaming(member, empty.below(), hasher);
                }
            }
        }
        JsonValue::Array(arr) => {
            1u8.hash(hasher);
            arr.len().hash(hasher);
            for item in arr {
                hash_json_streaming(item, empty.below(), hasher);
            }
        }
        // The one that matters: hashed in place, never copied or escaped.
        JsonValue::String(s) => {
            2u8.hash(hasher);
            s.hash(hasher);
        }
        JsonValue::Number(n) => {
            3u8.hash(hasher);
            n.to_string().hash(hasher);
        }
        JsonValue::Bool(b) => {
            4u8.hash(hasher);
            b.hash(hasher);
        }
        JsonValue::Null => 5u8.hash(hasher),
    }
}

/// True for a value that carries no information: `null`, `""`, `[]`, `{}`, or a container whose
/// every member is itself empty.
fn is_empty_json_member(value: &serde_json::Value) -> bool {
    use serde_json::Value as JsonValue;
    match value {
        JsonValue::Null => true,
        JsonValue::String(s) => s.trim().is_empty(),
        JsonValue::Array(arr) => arr.iter().all(is_empty_json_member),
        JsonValue::Object(map) => map.values().all(is_empty_json_member),
        _ => false,
    }
}
