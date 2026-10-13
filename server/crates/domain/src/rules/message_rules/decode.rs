//! The one decode step: a serialised member of an already-parsed value, read in place.
//!
//! Declared on a message rule (its read value), on an indexed family's `read` (each entry) and on an overlay (the
//! richer copy). All three call [`decode_member_in_place`] and are validated by [`decode_defect`], so the places a
//! decode is written cannot come to mean different things.

use super::*;

/// Decode the member `decode.select` names, in place: its text becomes the value it decodes to and its siblings
/// stay. Answers whether it decoded.
///
/// **Fail closed**: a member absent, not text, text the mode refuses, or text whose decoding leaves a value the
/// decode's `where` does not hold stays exactly what it was, never a partial structure. The Python and Java readers refuse
/// a text that does not print back byte for byte, so a truncated object (`content="This is the c...`) is left as
/// text. What an undecoded member means is the
/// caller's one decision: a rule's value or an overlay's copy declared to hold it is not that shape and reads
/// nothing, while an entry keeps the members it has - its role, and its content as the text it is - because the
/// family's other entries and that entry's own members are observations of their own.
pub(super) fn decode_member_in_place(value: &mut JsonValue, decode: &MemberDecode) -> bool {
    let Some(pointer) = decode.select.first_location(value) else {
        return false;
    };
    let Some(member) = value.pointer_mut(&pointer) else {
        return false;
    };
    let Some(decoded) = member
        .as_str()
        .and_then(|text| parse_value(text, decode.parse))
    else {
        return false;
    };
    // The condition is asked of the whole value with the member decoded, so it can name the member's siblings
    // (the entry's `role`) as well as what the member decoded to. Refused, the member is put back as it was.
    let text = std::mem::replace(member, decoded);
    if predicates_hold(value, &decode.require) {
        return true;
    }
    if let Some(member) = value.pointer_mut(&pointer) {
        *member = text;
    }
    false
}

/// What makes a declared decode mean nothing, wherever it is declared.
///
/// `$` would be the whole value, which the carrier's own `parse` decodes; `text` would keep the string a later
/// selection then finds nothing in.
pub(super) fn decode_defect(decode: &MemberDecode) -> Option<&'static str> {
    (decode.select.to_string() == "$" || decode.parse == ParseMode::Text).then_some(
        "a `decode` names a member and how it decodes: the whole value is the carrier's own `parse`, and a \
         member kept as text is a string no later selection can read through",
    )
}

/// Whether every decode a rule declares sits where something reads its result.
///
/// The rule's own `decode` rewrites the one value a single carrier parses to, so it means nothing on a rule that
/// assembles its value (a compose, a family, a branch set's parent) or splits the text another way (sections,
/// elements, a tool `repr`). An entry decode needs entries.
pub(super) fn decode_placement_defect(rule: &MessageRule) -> Option<&'static str> {
    let read = &rule.read;
    if let Some(decode) = &rule.decode {
        if rule.compose.is_some()
            || rule.branch_set.is_some()
            || rule.tool_repr.is_some()
            || rule.elements.is_some()
            || rule.sections.is_some()
            || read.indexed_family.is_some()
            || read.family.is_some()
        {
            return Some(
                "a rule's `decode` rewrites the value its one carrier parses to; a rule that assembles its \
                 value, splits its text or delegates to branches has no such value - an indexed family's \
                 entries declare theirs on `read`",
            );
        }
        if rule.parse == Some(ParseMode::Text) {
            return Some(
                "a rule's `decode` selects a member of its parsed value, and a carrier kept as `text` is a string \
                 with no members to select",
            );
        }
        if let Some(defect) = decode_defect(decode) {
            return Some(defect);
        }
    }
    if let Some(decode) = &read.decode {
        if read.indexed_family.is_none() {
            return Some("`read.decode` rewrites each entry of an indexed family, so it needs one");
        }
        if let Some(defect) = decode_defect(decode) {
            return Some(defect);
        }
    }
    read.overlay
        .as_ref()
        .and_then(|overlay| overlay.decode.as_ref())
        .and_then(decode_defect)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(select: &str, parse: &str) -> MemberDecode {
        serde_json::from_value(json!({"select": select, "parse": parse})).expect("a decode")
    }

    /// The rewrite lands where the path's first match sits, whatever the path's shape: inside a list, under a
    /// member whose name needs escaping as a JSON Pointer, and only at the first of several matches.
    #[test]
    fn the_first_match_is_rewritten_where_it_sits() {
        let mut value = json!({"items": [{"x": "1"}, {"a/b~c": "[2]", "x": "Said(text='cut..."}]});
        assert!(decode_member_in_place(
            &mut value,
            &decode("$.items[1]['a/b~c']", "json")
        ));
        assert!(decode_member_in_place(
            &mut value,
            &decode("$.items[*].x", "json")
        ));
        assert_eq!(
            value,
            json!({"items": [{"x": 1}, {"a/b~c": [2], "x": "Said(text='cut..."}]})
        );
        let before = value.clone();
        for (select, parse) in [
            ("$.missing", "json"),
            ("$.items", "json"),
            ("$.items[1].x", "python_repr"),
        ] {
            assert!(
                !decode_member_in_place(&mut value, &decode(select, parse)),
                "{select}"
            );
            assert_eq!(value, before, "{select}: left exactly as it was");
        }
    }

    /// Every shipped rule's placement stands as it is, and a rule decode added to one that assembles its value,
    /// splits its text, keeps it as text, or delegates to branches is refused - one case per refusal.
    #[test]
    fn a_rule_decode_is_refused_wherever_no_parsed_value_is_rewritten() {
        let mut refused = std::collections::BTreeSet::new();
        let mut accepted = 0;
        let mut seen = 0;
        for (path, bytes) in sideseat_rule_assets::sources() {
            let Ok(asset) = serde_json::from_slice::<JsonValue>(&bytes) else {
                continue;
            };
            let Some(rules) = asset.get("messages").and_then(JsonValue::as_array) else {
                continue;
            };
            for rule in rules {
                let parsed: MessageRule =
                    serde_json::from_value(rule.clone()).unwrap_or_else(|e| panic!("{path}: {e}"));
                seen += 1;
                assert_eq!(
                    decode_placement_defect(&parsed),
                    None,
                    "{path}: {}",
                    parsed.id
                );
                let mut with = rule.clone();
                with["decode"] = json!({"select": "$.member", "parse": "python_repr"});
                let with: MessageRule = serde_json::from_value(with).expect("still a rule");
                let kind = ["compose", "branch_set", "tool_repr", "elements", "sections"]
                    .into_iter()
                    .find(|key| rule.get(key).is_some())
                    .or_else(|| {
                        ["family", "indexed_family"]
                            .into_iter()
                            .find(|key| rule["read"].get(key).is_some())
                    })
                    .or_else(|| (rule.get("parse") == Some(&json!("text"))).then_some("text"));
                match kind {
                    Some(kind) => {
                        assert!(
                            decode_placement_defect(&with).is_some(),
                            "{path}: {} ({kind})",
                            parsed.id
                        );
                        refused.insert(kind);
                    }
                    None if decode_placement_defect(&with).is_none() => accepted += 1,
                    None => {}
                }
            }
        }
        assert!(
            seen > 0 && accepted > 0,
            "{seen} rules, {accepted} accepting a decode"
        );
        // `elements` has no shipped rule, so it is probed on its own.
        let elements: MessageRule = serde_json::from_value(json!({
            "id": "probe", "read": {"attribute": "a"}, "parse": "json",
            "elements": {"passes": []}, "decode": {"select": "$.member", "parse": "python_repr"},
        }))
        .expect("a probe");
        assert!(decode_placement_defect(&elements).is_some());
        refused.insert("elements");
        assert_eq!(
            refused.into_iter().collect::<Vec<_>>(),
            [
                "branch_set",
                "compose",
                "elements",
                "family",
                "indexed_family",
                "sections",
                "text",
                "tool_repr"
            ],
            "every refused placement is exercised"
        );
    }
}
