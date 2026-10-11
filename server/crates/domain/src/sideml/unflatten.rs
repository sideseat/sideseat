//! Unflattens dotted keys in JSON objects.
//!
//! OpenInference stores attributes as flat keys like:
//! `{"tool_calls.0.tool_call.function.name": "foo"}`
//!
//! This module converts them to nested structure:
//! `{"tool_calls": [{"tool_call": {"function": {"name": "foo"}}}]}`

use serde_json::Value as JsonValue;

/// Unflatten dotted keys in a JSON object into nested structure.
pub fn unflatten_dotted_keys(value: &JsonValue) -> JsonValue {
    let JsonValue::Object(map) = value else {
        return value.clone();
    };

    // Separate dotted keys from regular keys
    let mut regular = serde_json::Map::new();
    let mut dotted: Vec<(&String, &JsonValue)> = Vec::new();

    for (key, val) in map {
        if key.contains('.') {
            dotted.push((key, val));
        } else {
            regular.insert(key.clone(), val.clone());
        }
    }

    // If no dotted keys, return as-is
    if dotted.is_empty() {
        return value.clone();
    }

    // Process dotted keys. Two screens keep a key flat, losslessly, before anything is created:
    //
    // - **Canonical indices only.** A segment that parses as an index but is not spelled canonically -
    //   `01`, a sign - folded onto another key's entry: `contents.01` overwrote `contents.1`'s block, which
    //   of them won following the map's order.
    // - **The gap budget.** A dotted key names its position outright, so one `contents.1000000000.text`
    //   otherwise creates a billion null gap cells. The gap is computed *before* any mutation - a refused
    //   walk used to leave empty containers it had invented on the way in, which downstream readers then
    //   took for members the producer sent.
    let mut gap_budget = sideseat_core::constants::UNFLATTEN_MAX_GAP_CELLS;
    for (dotted_key, val) in dotted {
        let canonical = dotted_key
            .split('.')
            .all(|segment| segment.parse::<usize>().is_err() || canonical_index(segment).is_some());
        let gap = canonical
            .then(|| gap_to_set(&regular, dotted_key))
            .flatten()
            .filter(|gap| *gap <= gap_budget);
        let Some(gap) = gap else {
            tracing::warn!(
                target: "sideseat::sideml",
                key = %dotted_key,
                budget = sideseat_core::constants::UNFLATTEN_MAX_GAP_CELLS,
                "a dotted key spells a non-canonical index or names one past this payload's gap budget; \
                 it is kept flat"
            );
            regular.insert(dotted_key.clone(), val.clone());
            continue;
        };
        gap_budget -= gap;
        set_nested_value(&mut regular, dotted_key, val.clone());
    }

    JsonValue::Object(regular)
}

/// The index a segment spells, when it spells one canonically: decimal digits, no sign, no leading zero.
fn canonical_index(segment: &str) -> Option<usize> {
    let canonical = !segment.is_empty()
        && segment.bytes().all(|byte| byte.is_ascii_digit())
        && (segment == "0" || !segment.starts_with('0'));
    canonical.then(|| segment.parse().ok()).flatten()
}

/// The null gap cells setting this path would create, mirroring [`set_nested_value`]'s walk read-only, or
/// `None` where the arithmetic cannot say (an index at `usize::MAX`). Computed before any mutation, so a
/// refusal creates nothing.
fn gap_to_set(root: &serde_json::Map<String, JsonValue>, path: &str) -> Option<usize> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut acc: usize = 0;
    // `None` once the walk is past what exists: everything deeper is created fresh.
    let mut cursor: Option<&serde_json::Map<String, JsonValue>> = Some(root);
    let mut i = 0;
    while i < parts.len() {
        let part = parts[i];
        let is_last = i == parts.len() - 1;
        if is_last || part.parse::<usize>().is_ok() {
            return Some(acc);
        }
        let next_index = parts.get(i + 1).and_then(|p| canonical_index(p));
        let child = cursor.and_then(|map| map.get(part));
        match (child, next_index) {
            // An existing or fresh array: the gap is up to the index, less what the array already holds.
            (found, Some(idx)) => {
                let len = match found {
                    Some(JsonValue::Array(arr)) => arr.len(),
                    Some(_) => return Some(acc),
                    None => 0,
                };
                acc = acc.saturating_add(idx.checked_add(1)?.saturating_sub(len));
                // Into the element: an existing object continues the walk, anything else is fresh.
                cursor = match found {
                    Some(JsonValue::Array(arr)) => match arr.get(idx) {
                        Some(JsonValue::Object(obj)) => Some(obj),
                        Some(JsonValue::Null) | None => None,
                        Some(_) => return Some(acc),
                    },
                    _ => None,
                };
                i += 2;
            }
            // An object level: descend where it exists, go fresh where it does not.
            (found, None) => {
                cursor = match found {
                    Some(JsonValue::Object(obj)) => Some(obj),
                    Some(_) => return Some(acc),
                    None => None,
                };
                i += 1;
            }
        }
    }
    Some(acc)
}

/// Set a value at a nested path, creating intermediate objects/arrays as needed. The caller has already
/// screened the path - canonical indices, the gap budget - so this walk is unconditional.
fn set_nested_value(root: &mut serde_json::Map<String, JsonValue>, path: &str, value: JsonValue) {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return;
    }

    let mut current = root;

    for (i, part) in parts.iter().enumerate() {
        let is_last = i == parts.len() - 1;
        let next_is_index = parts
            .get(i + 1)
            .is_some_and(|p| canonical_index(p).is_some());

        if is_last {
            current.insert(part.to_string(), value);
            return;
        }

        // Check if this part is a numeric index - skip to array handling
        if part.parse::<usize>().is_ok() {
            return;
        }

        // Ensure the key exists with correct type
        let entry = current.entry(part.to_string()).or_insert_with(|| {
            if next_is_index {
                JsonValue::Array(Vec::new())
            } else {
                JsonValue::Object(serde_json::Map::new())
            }
        });

        // Navigate into the next level
        match entry {
            JsonValue::Object(obj) => {
                current = obj;
            }
            JsonValue::Array(arr) => {
                // Next part should be an index
                if let Some(next_part) = parts.get(i + 1)
                    && let Some(idx) = canonical_index(next_part)
                {
                    // Ensure array has enough elements. A gap is `null`: an index no key named holds nothing,
                    // and an empty object would be indistinguishable from a member the producer sent empty -
                    // a structured output of `{}` - which the readers downstream then had to guess at. The
                    // caller's gap budget already screened these pushes.
                    while arr.len() <= idx {
                        arr.push(JsonValue::Null);
                    }
                    // Set remaining path on array element
                    let remaining_path = parts[i + 2..].join(".");
                    if remaining_path.is_empty() {
                        arr[idx] = value;
                    } else {
                        if arr[idx].is_null() {
                            arr[idx] = JsonValue::Object(serde_json::Map::new());
                        }
                        if let JsonValue::Object(ref mut obj) = arr[idx] {
                            set_nested_value(obj, &remaining_path, value);
                        }
                    }
                }
                return;
            }
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A gap in an indexed family is `null`, never `{}`: an empty object is a value a producer can send, and
    /// filling gaps with one made the two indistinguishable downstream.
    #[test]
    fn a_gap_between_indices_is_null() {
        let flat = json!({
            "contents.1.message_content.text": "hello",
            "parts.0": "first",
            "parts.2": "third"
        });
        let nested = unflatten_dotted_keys(&flat);
        assert_eq!(
            nested["contents"],
            json!([null, {"message_content": {"text": "hello"}}])
        );
        assert_eq!(nested["parts"], json!(["first", null, "third"]));
    }
}

/// A dotted key names its position outright, so one key can otherwise demand a billion null gap cells. Past
/// the shared budget the key is kept flat - the reading preserved, never expanded - and keys within the
/// budget are untouched by one that is not.
#[cfg(test)]
mod gap_budget_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_refused_key_creates_nothing_at_any_depth() {
        // The gap is computed before any mutation: a refused nested walk used to leave `{"meta": []}` behind,
        // which downstream readers took for a member the producer sent.
        let flat = json!({"content.meta.1000000000.text": "x"});
        let out = unflatten_dotted_keys(&flat);
        assert!(out.get("content").is_none(), "{out}");
        assert_eq!(out["content.meta.1000000000.text"], "x");

        // The arithmetic boundary: an index at usize::MAX wrapped `idx + 1` to zero and passed the budget.
        let flat = json!({"contents.18446744073709551615.text": "x"});
        let out = unflatten_dotted_keys(&flat);
        assert_eq!(out["contents.18446744073709551615.text"], "x", "{out}");
        assert!(out.get("contents").is_none());
    }

    #[test]
    fn a_non_canonical_index_keeps_its_key_flat() {
        // `01` folded onto `1`'s entry, and which won followed the map's order.
        let flat = json!({
            "contents.1.text": "first",
            "contents.01.text": "sneak"
        });
        let out = unflatten_dotted_keys(&flat);
        assert_eq!(out["contents"][1]["text"], "first", "{out}");
        assert_eq!(out["contents.01.text"], "sneak", "{out}");
    }

    #[test]
    fn an_index_past_the_gap_budget_keeps_its_key_flat() {
        let flat = json!({
            "contents.0.text": "first",
            "contents.1000000000.text": "unreachable"
        });
        let out = unflatten_dotted_keys(&flat);
        assert_eq!(out["contents"][0]["text"], "first");
        assert_eq!(
            out["contents.1000000000.text"], "unreachable",
            "the unreachable key is preserved as it stands: {out}"
        );
        assert_eq!(
            out["contents"].as_array().map(Vec::len),
            Some(1),
            "no gap cells were created for it"
        );

        // The budget is shared: many keys cannot allocate what one may not.
        let mut map = serde_json::Map::new();
        for n in 0..64 {
            map.insert(format!("a{n}.60000.v"), json!(n));
        }
        let out = unflatten_dotted_keys(&JsonValue::Object(map));
        let expanded = (0..64)
            .filter(|n| out.get(format!("a{n}")).is_some())
            .count();
        let kept_flat = (0..64)
            .filter(|n| out.get(format!("a{n}.60000.v")).is_some())
            .count();
        assert_eq!(expanded + kept_flat, 64, "{out}");
        assert!(
            expanded >= 1 && kept_flat >= 62,
            "expanded {expanded}, flat {kept_flat}"
        );
    }
}
