use super::*;

/// Whether one predicate holds of a value.
/// One value predicate, reachable from the test that pins what `non_empty` means for a scalar.
#[cfg(test)]
pub(crate) fn predicate_holds_for_test(predicate: &ValuePredicate, value: &JsonValue) -> bool {
    predicate_holds(value, predicate)
}

#[cfg(test)]
pub(super) fn predicate_holds(value: &JsonValue, predicate: &ValuePredicate) -> bool {
    // A path that can match more than once is asked **existentially**: some match satisfies the condition.
    // Answering only about the first is a silent narrowing, and it would disagree with `exists`, which on a
    // filter path already means "at least one".
    if let Some(path) = &predicate.path {
        let matches = query(value, path);
        if matches.len() > 1 {
            return matches
                .into_iter()
                .any(|subject| condition_holds(subject, predicate));
        }
    }
    let Some(subject) = (match &predicate.path {
        Some(path) => singular(value, path, "reading select"),
        None => Some(value),
    }) else {
        // Absent. Only a predicate asserting absence is satisfied - or one asserting the value is not in a
        // set, which is true when there is no value, and is how a dialect's unnamed events fall through to
        // the reading that handles them.
        return predicate.exists == Some(false)
            || (!predicate.none_of.is_empty()
                && predicate.exists.is_none()
                && predicate.kind.is_none()
                && predicate.non_empty.is_none()
                && predicate.not_null.is_none()
                && predicate.identifier_like.is_none()
                && predicate.starts_with.is_none()
                && predicate.lacks_prefix.is_none()
                && predicate.one_of.is_empty()
                && predicate.equals.is_none()
                && predicate.only_members.is_empty());
    };
    condition_holds(subject, predicate)
}

#[cfg(test)]
/// The conditions a *present* value must satisfy.
pub(super) fn condition_holds(subject: &JsonValue, predicate: &ValuePredicate) -> bool {
    if predicate.exists == Some(false) {
        return false;
    }
    if let Some(kind) = predicate.kind
        && !matches_kind(subject, kind)
    {
        return false;
    }
    if let Some(want_identifier) = predicate.identifier_like {
        let looks_like_one = subject
            .as_str()
            .is_some_and(|text| text.starts_with(|c: char| c.is_alphanumeric() || c == '_'));
        if looks_like_one != want_identifier {
            return false;
        }
    }
    if let Some(want_not_null) = predicate.not_null
        && subject.is_null() == want_not_null
    {
        return false;
    }
    if let Some(want_non_empty) = predicate.non_empty {
        let filled = match subject {
            JsonValue::String(text) => !text.is_empty(),
            JsonValue::Array(items) => !items.is_empty(),
            JsonValue::Object(map) => !map.is_empty(),
            // A scalar is neither empty **nor** non-empty in this vocabulary, so it fails the predicate
            // whichever way it was asked. `true` here made `{"path": "$.content", "non_empty": true}` hold
            // for `{"content": 0}` - a number reported as filled content - and the compiler's refusal does
            // not cover it, because the refusal is about the *declaration* and this is about the value.
            _ => return false,
        };
        if filled != want_non_empty {
            return false;
        }
    }
    if let Some(prefix) = &predicate.starts_with
        && !subject
            .as_str()
            .is_some_and(|text| text.starts_with(prefix.as_str()))
    {
        return false;
    }
    if let Some(prefix) = &predicate.lacks_prefix
        && subject
            .as_str()
            .is_some_and(|text| text.starts_with(prefix.as_str()))
    {
        return false;
    }
    if !predicate.one_of.is_empty()
        && !subject
            .as_str()
            .is_some_and(|text| predicate.one_of.iter().any(|want| want == text))
    {
        return false;
    }
    if !predicate.none_of.is_empty()
        && subject
            .as_str()
            .is_some_and(|text| predicate.none_of.iter().any(|reject| reject == text))
    {
        return false;
    }
    if predicate
        .equals
        .as_ref()
        .is_some_and(|value| subject != value)
    {
        return false;
    }
    if !predicate.only_members.is_empty()
        && !subject.as_object().is_some_and(|members| {
            members
                .keys()
                .all(|key| predicate.only_members.iter().any(|name| name == key))
        })
    {
        return false;
    }
    true
}

#[cfg(test)]
pub(super) fn matches_kind(value: &JsonValue, kind: ValueKind) -> bool {
    match kind {
        ValueKind::Object => value.is_object(),
        ValueKind::Array => value.is_array(),
        ValueKind::String => value.is_string(),
        ValueKind::Number => value.is_number(),
        ValueKind::Bool => value.is_boolean(),
        ValueKind::Null => value.is_null(),
    }
}

/// Whether a predicate set holds of a value. An empty set holds.
pub(in crate::rules) fn predicates_hold(value: &JsonValue, condition: &ValueCondition) -> bool {
    match condition.expression() {
        // Nothing declared places no condition, so it holds - a different answer from an expression that is
        // false, which is the distinction `Truth` exists to keep.
        None => true,
        Some(expr) => expr.eval(&mut |atom| atom.eval(value)).holds(),
    }
}

/// The observations an array-valued carrier yields, pass by pass.
///
/// Each pass scans every element. That is the shape of the code being replaced and the order is
/// observable - one dialect emits every recognised event before any grouped block - so it is declared
/// rather than left to how a loop happens to be written.
/// Each element pass's observations, with **which clauses produced them**.
///
/// The path is the pass, and for a grouped pass the derived case whose predicate matched - both are required
/// declarations that this function used to discard, leaving two routes of one rule indistinguishable in a
/// diagnostic.
pub(super) fn element_passes(
    parsed: &JsonValue,
    spec: &ElementsSpec,
) -> Vec<(String, JsonValue, Vec<Vec<String>>)> {
    // The elements are the parsed value itself: every shipped dialect packs them as a root array.
    let Some(items) = parsed.as_array() else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for pass in &spec.passes {
        let matching = items
            .iter()
            .filter(|element| predicates_hold(element, &pass.when));

        match &pass.group {
            // Runs of consecutive elements sharing a derived key.
            Some(group) => {
                let mut run_key: Option<String> = None;
                let mut collected: Vec<JsonValue> = Vec::new();
                // The case that produced the run travels with it: a run is keyed by a *derived value*, and
                // several cases may derive the same one, so the key alone does not name the clause.
                let flush = |key: Option<String>,
                             cases: Vec<String>,
                             blocks: Vec<JsonValue>,
                             out: &mut Vec<_>| {
                    let Some(key) = key else { return };
                    if blocks.is_empty() {
                        return;
                    }
                    let Some(tag) = group.tag_by_key.get(&key) else {
                        return;
                    };
                    // One path per contributing case, each under this pass.
                    let paths: Vec<Vec<String>> = if cases.is_empty() {
                        vec![vec![pass.id.clone()]]
                    } else {
                        cases
                            .into_iter()
                            .map(|case| vec![pass.id.clone(), case])
                            .collect()
                    };
                    out.push((
                        tag.clone(),
                        json!({
                            group.key_as.clone(): key,
                            "content": JsonValue::Array(blocks),
                        }),
                        paths,
                    ));
                };
                // **Every** contributing case, not the first: two cases deriving one key are legitimate
                // aliases, so a run built from both has two witnesses and naming one of them claims it
                // produced blocks it did not match.
                let mut run_cases: Vec<String> = Vec::new();
                // **The whole array**, not the pass's matches: "consecutive" is a fact about the array a
                // producer wrote, and filtering first made it a fact about the filtered view. Logfire's shape -
                // two content blocks with a named assistant event between them - collapsed into one "run" of the
                // two, with a message lying between them that the run claims is not there. So an element that
                // fails the pass, derives no case, or has nothing to collect **ends** the current run.
                for element in items {
                    let Some(matched) = Some(element)
                        .filter(|element| predicates_hold(element, &pass.when))
                        .and_then(|element| {
                            group
                                .by
                                .iter()
                                .find(|case| predicates_hold(element, &case.when))
                        })
                    else {
                        flush(
                            run_key.take(),
                            std::mem::take(&mut run_cases),
                            std::mem::take(&mut collected),
                            &mut out,
                        );
                        continue;
                    };
                    let key = matched.value.clone();
                    let Some(part) = singular(element, &group.collect, "group collect") else {
                        flush(
                            run_key.take(),
                            std::mem::take(&mut run_cases),
                            std::mem::take(&mut collected),
                            &mut out,
                        );
                        continue;
                    };
                    if run_key.as_ref() != Some(&key) {
                        flush(
                            run_key.take(),
                            std::mem::take(&mut run_cases),
                            std::mem::take(&mut collected),
                            &mut out,
                        );
                        run_key = Some(key);
                    }
                    if !run_cases.iter().any(|seen| seen == &matched.id) {
                        run_cases.push(matched.id.clone());
                    }
                    collected.push(part.clone());
                }
                flush(run_key, run_cases, collected, &mut out);
            }
            // Each element emitted as it stands, tagged by what it carries.
            None => {
                let Some(tag_path) = &pass.tag_from else {
                    continue;
                };
                for element in matching {
                    let Some(tag) = query(element, tag_path)
                        .into_iter()
                        .next()
                        .and_then(JsonValue::as_str)
                    else {
                        continue;
                    };
                    out.push((
                        tag.to_string(),
                        element.clone(),
                        vec![vec![pass.id.clone()]],
                    ));
                }
            }
        }
    }
    out
}
