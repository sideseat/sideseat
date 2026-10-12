//! Unflattens dotted keys in JSON objects.
//!
//! OpenInference stores attributes as flat keys like:
//! `{"tool_calls.0.tool_call.function.name": "foo"}`
//!
//! This module converts them to nested structure:
//! `{"tool_calls": [{"tool_call": {"function": {"name": "foo"}}}]}`

use std::collections::{HashMap, HashSet};

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
    // The null cells this function created as gaps, as index sets per array - the array's path stored once,
    // so a long prefix with many gaps costs its length once and a word per gap. Every other `null` is a value a
    // producer wrote - a key spelling `a.0` as null, or a regular member holding `[null]` - and filling or
    // descending into it would overwrite that value, so only a recorded gap may be filled.
    let mut gaps: Gaps = HashMap::new();
    for (dotted_key, val) in dotted {
        let canonical = dotted_key
            .split('.')
            .all(|segment| segment.parse::<usize>().is_err() || canonical_index(segment).is_some());
        let parts: Vec<&str> = dotted_key.split('.').collect();
        let gap = match canonical.then(|| placement(&regular, &parts, &gaps)) {
            Some(Placement::Gap(gap)) if gap <= gap_budget => gap,
            _ => {
                tracing::warn!(
                    target: "sideseat::sideml",
                    key = %dotted_key,
                    budget = sideseat_core::constants::UNFLATTEN_MAX_GAP_CELLS,
                    "a dotted key cannot be placed - a non-canonical index, a conflict, the segment \
                     ceiling or the gap budget - so it is kept flat"
                );
                regular.insert(dotted_key.clone(), val.clone());
                continue;
            }
        };
        gap_budget -= gap;
        set_nested_value(&mut regular, &parts, val.clone(), &mut gaps);
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

/// The gap cells created so far: per array, by its path `parts[..=i]` joined, the indices left `null`.
type Gaps = HashMap<String, HashSet<usize>>;

/// Whether cell `idx` of the array at `parts[..=i]` is a gap this function created.
fn is_gap(gaps: &Gaps, parts: &[&str], i: usize, idx: usize) -> bool {
    gaps.get(&parts[..=i].join("."))
        .is_some_and(|cells| cells.contains(&idx))
}

/// Where a dotted key's value can go.
enum Placement {
    /// It fits, creating this many null gap cells.
    Gap(usize),
    /// It cannot be placed without losing something - a container of the wrong kind in the way, a
    /// destination another key already filled, arithmetic past `usize`, or a path past the segment
    /// ceiling - so the key stays flat, losslessly.
    Blocked,
}

/// Mirror [`set_nested_value`]'s walk read-only, before any mutation: a refusal creates nothing, and a
/// conflict is kept flat rather than silently dropped or overwritten.
fn placement(root: &serde_json::Map<String, JsonValue>, parts: &[&str], gaps: &Gaps) -> Placement {
    if parts.len() > sideseat_core::constants::UNFLATTEN_MAX_SEGMENTS {
        return Placement::Blocked;
    }
    let mut acc: usize = 0;
    // `None` once the walk is past what exists: everything deeper is created fresh and conflicts with
    // nothing.
    let mut cursor: Option<&serde_json::Map<String, JsonValue>> = Some(root);
    let mut i = 0;
    while i < parts.len() {
        let part = parts[i];
        if i == parts.len() - 1 {
            // The destination member: a value another key already placed there would be overwritten.
            return match cursor.is_some_and(|map| map.contains_key(part)) {
                true => Placement::Blocked,
                false => Placement::Gap(acc),
            };
        }
        if part.parse::<usize>().is_ok() {
            // A numeric segment where a member name belongs: the setter places nothing, so the key must
            // stay flat rather than vanish.
            return Placement::Blocked;
        }
        let next_index = parts.get(i + 1).and_then(|p| canonical_index(p));
        let child = cursor.and_then(|map| map.get(part));
        match (child, next_index) {
            // An existing or fresh array: the gap is up to the index, less what the array already holds.
            (found, Some(idx)) => {
                let len = match found {
                    Some(JsonValue::Array(arr)) => arr.len(),
                    Some(_) => return Placement::Blocked,
                    None => 0,
                };
                let Some(needed) = idx.checked_add(1) else {
                    return Placement::Blocked;
                };
                acc = acc.saturating_add(needed.saturating_sub(len));
                let element = match found {
                    Some(JsonValue::Array(arr)) => arr.get(idx),
                    _ => None,
                };
                // A `null` is open only where this function left it as a gap; any other is a producer's.
                let open_null =
                    matches!(element, Some(JsonValue::Null)) && is_gap(gaps, parts, i, idx);
                if i + 2 == parts.len() {
                    // The destination element: filled by another key already means overwritten.
                    return match element {
                        None => Placement::Gap(acc),
                        Some(JsonValue::Null) if open_null => Placement::Gap(acc),
                        Some(_) => Placement::Blocked,
                    };
                }
                cursor = match element {
                    Some(JsonValue::Object(obj)) => Some(obj),
                    None => None,
                    Some(JsonValue::Null) if open_null => None,
                    Some(_) => return Placement::Blocked,
                };
                i += 2;
            }
            // An object level: descend where it exists, go fresh where it does not, and refuse a container
            // of the wrong kind - the setter walked into it and dropped the value.
            (found, None) => {
                cursor = match found {
                    Some(JsonValue::Object(obj)) => Some(obj),
                    Some(_) => return Placement::Blocked,
                    None => None,
                };
                i += 1;
            }
        }
    }
    Placement::Gap(acc)
}

/// Set a value at a nested path, creating intermediate objects/arrays as needed. The caller has already
/// screened the path - canonical indices, the segment ceiling, the gap budget, conflicts - so this walk is
/// unconditional. Iterative, with the parts split once: joining and re-splitting the remainder at every
/// array level copied the path quadratically.
fn set_nested_value(
    root: &mut serde_json::Map<String, JsonValue>,
    parts: &[&str],
    value: JsonValue,
    gaps: &mut Gaps,
) {
    let mut current = root;
    let mut i = 0;
    while i < parts.len() {
        let part = parts[i];
        if i == parts.len() - 1 {
            current.insert(part.to_string(), value);
            return;
        }
        let next_index = parts.get(i + 1).and_then(|p| canonical_index(p));
        let entry = current.entry(part.to_string()).or_insert_with(|| {
            if next_index.is_some() {
                JsonValue::Array(Vec::new())
            } else {
                JsonValue::Object(serde_json::Map::new())
            }
        });
        match entry {
            JsonValue::Object(obj) => {
                current = obj;
                i += 1;
            }
            JsonValue::Array(arr) => {
                let Some(idx) = next_index else { return };
                // A gap is `null`: an index no key named holds nothing, and an empty object would be
                // indistinguishable from a member the producer sent empty - a structured output of `{}` -
                // which the readers downstream then had to guess at. The caller's budget screened these.
                let cells = gaps.entry(parts[..=i].join(".")).or_default();
                while arr.len() <= idx {
                    cells.insert(arr.len());
                    arr.push(JsonValue::Null);
                }
                cells.remove(&idx);
                if i + 2 == parts.len() {
                    arr[idx] = value;
                    return;
                }
                if arr[idx].is_null() {
                    arr[idx] = JsonValue::Object(serde_json::Map::new());
                }
                let JsonValue::Object(obj) = &mut arr[idx] else {
                    return;
                };
                current = obj;
                i += 2;
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
    fn a_conflicting_or_bottomless_key_stays_flat() {
        // A container of the wrong kind in the way: the setter used to walk in and drop the value.
        let out = unflatten_dotted_keys(&json!({"content": "original", "content.text": "kept"}));
        assert_eq!(out["content"], "original");
        assert_eq!(out["content.text"], "kept", "{out}");

        // A destination another key already filled: overwriting it lost the producer's own member.
        let out = unflatten_dotted_keys(
            &json!({"content": {"text": "original"}, "content.text": "second"}),
        );
        assert_eq!(out["content"]["text"], "original");
        assert_eq!(out["content.text"], "second", "{out}");

        // A key past the segment ceiling builds no tower of containers.
        let deep = format!("content.{}text", "a.0.".repeat(20_000));
        let mut map = serde_json::Map::new();
        map.insert(deep.clone(), json!("x"));
        let out = unflatten_dotted_keys(&JsonValue::Object(map));
        assert_eq!(out[&deep], "x");
        assert!(out.get("content").is_none(), "nothing was built");
    }

    #[test]
    fn a_producer_null_is_a_value_not_a_gap() {
        // A key spelling the element as null: a later key descending into it would overwrite it.
        let out = unflatten_dotted_keys(&json!({"a.0": null, "a.0.text": "x"}));
        assert_eq!(out["a"], json!([null]), "{out}");
        assert_eq!(out["a.0.text"], "x", "{out}");

        // A regular member holding a null element: a dotted key filling it would overwrite it.
        let out = unflatten_dotted_keys(&json!({"a": [null], "a.0": "x"}));
        assert_eq!(out["a"], json!([null]), "{out}");
        assert_eq!(out["a.0"], "x", "{out}");

        // A gap this function created is still open to the key that names it: `a.10` sorts before `a.2`, so
        // the first key leaves gaps and the second fills one.
        let out = unflatten_dotted_keys(&json!({"a.10": "k", "a.2": "c"}));
        assert_eq!(out["a"][2], "c", "{out}");
        assert_eq!(out["a"][10], "k", "{out}");
        assert!(out.get("a.2").is_none(), "{out}");
    }

    #[test]
    fn a_long_prefix_with_many_gaps_costs_its_length_once() {
        // One key, 64 KiB of prefix, 65,535 gaps: a string per gap holding the whole prefix was four
        // gigabytes of bookkeeping from a payload of 64 KiB. Per-array index sets hold the prefix once.
        let key = format!("{}.65535", "a".repeat(65_536));
        let mut map = serde_json::Map::new();
        map.insert(key.clone(), json!("x"));
        let out = unflatten_dotted_keys(&JsonValue::Object(map));
        let cells = out["a".repeat(65_536).as_str()]
            .as_array()
            .expect("the array was built");
        assert_eq!(cells.len(), 65_536);
        assert_eq!(cells[65_535], "x");
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

/// Whatever the keys spell, nothing is lost: every dotted value is either placed where its path says or kept
/// under its flat key, and never both. This is the mirror invariant between [`placement`] and
/// [`set_nested_value`]: a drift between the two either loses a value (the screen allowed what the setter
/// could not place) or keeps one twice.
#[cfg(test)]
mod placement_props {
    use super::*;
    use proptest::prelude::*;

    fn resolve<'v>(mut node: &'v JsonValue, parts: &[&str]) -> Option<&'v JsonValue> {
        for part in parts {
            node = match node {
                JsonValue::Object(map) => map.get(*part)?,
                // Canonically, as the function reads indices: `00` names no element.
                JsonValue::Array(arr) => arr.get(canonical_index(part)?)?,
                _ => return None,
            };
        }
        Some(node)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]
        #[test]
        fn every_dotted_value_survives_exactly_once(
            keys in proptest::collection::btree_map(
                proptest::collection::vec(
                    proptest::sample::select(vec!["a", "b", "0", "1", "2", "00", "x"]),
                    1..5,
                )
                .prop_map(|segments| segments.join(".")),
                proptest::option::of(0u32..100),
                1..8,
            )
        ) {
            let mut map = serde_json::Map::new();
            // `None` is a producer's null, which must survive like any other value.
            for (key, value) in &keys {
                map.insert(key.clone(), serde_json::json!(value));
            }
            let out = unflatten_dotted_keys(&JsonValue::Object(map));
            for (key, value) in &keys {
                if !key.contains('.') {
                    prop_assert_eq!(&out[key], &serde_json::json!(value), "a plain key stays: {}", key);
                    continue;
                }
                let parts: Vec<&str> = key.split('.').collect();
                let flat = out.get(key.as_str());
                let nested = resolve(&out, &parts);
                let placed = nested.is_some_and(|found| found == &serde_json::json!(value));
                let kept = flat.is_some_and(|found| found == &serde_json::json!(value));
                prop_assert!(
                    placed ^ kept,
                    "key {} must survive exactly once (placed {}, kept flat {}): {}",
                    key, placed, kept, out
                );
            }
        }
    }
}
