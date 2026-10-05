use super::*;

/// Inline each reading's fragment reference, so the runtime holds cases rather than a name.
pub(super) fn inline_fragments(
    readings: &[Alternative],
    fragments: &HashMap<String, Vec<Alternative>>,
) -> Result<Vec<CompiledReading>, MessageCompileError> {
    readings
        .iter()
        .map(|spec| {
            let mut fragment_cases = match &spec.then_fragment {
                Some(name) => match fragments.get(name) {
                    Some(cases) => cases.clone(),
                    None => {
                        return Err(MessageCompileError::UnknownFragment {
                            fragment: name.clone(),
                        });
                    }
                },
                None => Vec::new(),
            };
            // After the shared table, never before: the table is the dialect's own answer and this is one
            // place that accepts one more shape.
            fragment_cases.extend(spec.extra_cases.iter().cloned());
            // A case is a *leaf*. `Alternative` is one type, so it structurally permits a nested
            // `then_fragment` or `extra_cases` - and a leaf's own `fragment_cases` is forced empty when it
            // runs, so such a declaration is silently ignored. Refused rather than left to be discovered,
            // which is the same reason a fragment may not reference a fragment.
            if let Some(nested) = fragment_cases
                .iter()
                .find(|case| case.then_fragment.is_some() || !case.extra_cases.is_empty())
            {
                // Named by the offending *case*, not by the reading that holds it: an `extra_cases` entry on
                // an unnamed reading was reported as "a reading", which does not locate anything.
                return Err(MessageCompileError::Inexpressible {
                    rule: nested
                        .doc
                        .clone()
                        .or_else(|| spec.then_fragment.clone())
                        .unwrap_or_else(|| "an undocumented case".to_string()),
                    detail: "a fragment case or extra case is a leaf, so a `then_fragment` or \
                             `extra_cases` on it would be ignored",
                });
            }
            // **Traversal slots that the pipeline order makes dead.** Each of these compiled and did nothing,
            // which is the same defect as a construction branch accepting a sibling it returns before:
            //
            // | Declared | What happens |
            // | --- | --- |
            // | a lift `from: element` with no `descend` | the element *is* the candidate, so every member is already there |
            // | `else_element` with no `then_present_any_of` | it names the fallback for a coalesce that is not there |
            // A lift **from the element** copies members that sit beside the value being emitted, which is
            // only a different value when something was descended into: without `descend` the element *is* the
            // candidate, so every member is already there and the lift is a no-op.
            if spec.descend.is_none()
                && spec
                    .lift
                    .iter()
                    .any(|lift| lift.from == super::schema::LiftSource::Element)
            {
                return Err(MessageCompileError::Inexpressible {
                    rule: spec.id.clone(),
                    detail: "lifts from the element with no `descend`, where the element is already the value \
                             being emitted - so every member is there and the lift copies nothing",
                });
            }
            if spec.else_element && spec.then_present_any_of.is_empty() {
                return Err(MessageCompileError::Inexpressible {
                    rule: spec.id.clone(),
                    detail: "declares `else_element` with no `then_present_any_of` - it names the fallback \
                             for a coalesce that is not there",
                });
            }
            Ok(CompiledReading {
                spec: spec.clone(),
                fragment_cases,
            })
        })
        .collect()
}

/// The readings of every node of a bounded walk.
///
/// Bounded three ways, all declared: the depth, the members pruned because the node-level readings already
/// took them, and stopping below a node that was itself read as a message - a message's members are its
/// content, so descending into one would read its parts as turns.
pub(super) fn walked_readings(
    root: &JsonValue,
    rule: &CompiledMessageRule,
    walk: &super::schema::WalkSpec,
    build: Option<Construction<'_>>,
) -> Vec<Reading> {
    let mut out = Vec::new();
    let mut stack = vec![(root, walk.max_depth)];
    // **Work is bounded, not just depth.** A rule declares how deep to descend, which is semantics - the shape
    // of the state object a framework writes. How much work that may cost against an adversarial payload is
    // this server's business, and a ceiling an asset could raise would not be a ceiling. Within a declared depth
    // a payload nests as widely as it likes and every node is evaluated, so the depth alone bounds nothing; the
    // 64 MiB body limit does not either, because the cost is in the evaluation rather than the bytes.
    let mut visited = 0usize;
    while let Some((node, depth)) = stack.pop() {
        visited += 1;
        if visited > sideseat_core::constants::RULE_WALK_MAX_NODES {
            // Reported and **stopped**, rather than returning whatever was gathered: a truncated walk is a
            // partial answer that looks like a complete one, which is the class of defect this review keeps
            // finding. What has been read is still returned, because discarding it would lose messages a
            // producer really sent - so the loss is stated here rather than hidden either way.
            tracing::warn!(
                target: "sideseat::rules",
                rule = %rule.rule_id,
                limit = sideseat_core::constants::RULE_WALK_MAX_NODES,
                "a rule's walk reached this server's node ceiling; the rest of the payload was not visited"
            );
            break;
        }
        // **One pass, two answers.** Whether to descend is a question about the *payload's shape* - "this node
        // is the message, do not read its parts as turns" - while whether an envelope can be built is about the
        // declaration, so a clause whose construction failed still means the node is that shape. Deciding the
        // walk from the built result made construction failure widen the traversal, which the
        // carrier-subsequence invariant caught on `langgraph/image_gen`.
        //
        // It was two evaluations of the node for a while, which is quadratic work on a deep payload; `Selection`
        // returns both from one.
        //
        // And **which** clauses stop it is declared (`stop_on`), because "did anything get selected here" is a
        // wider question than "was this node a message": LangGraph's `also_3` selects every state member, so a
        // node holding a message *beside* more state counted as matched and its siblings were never visited.
        let here = all_readings(node, rule, build);
        let matched = here
            .recognised
            .iter()
            .any(|id| walk.stop_on.iter().any(|stop| stop == id));
        out.extend(here.built);
        if depth == 0 || matched {
            continue;
        }
        match node {
            JsonValue::Object(map) => {
                // A fixed order: the payload's own map order is what a reader sees, and the stack pops in
                // reverse, so members are pushed reversed to visit them as written.
                let members: Vec<&JsonValue> = map
                    .iter()
                    .filter(|(key, value)| {
                        // Pruned only where the clause that consumes this member actually recognised
                        // something here. As an unconditional name list, a member nothing read was skipped
                        // because of what it is called.
                        let taken = walk.prune.iter().any(|pruned| {
                            &pruned.member == *key && here.recognised.contains(&pruned.taken_by)
                        });
                        !taken && (value.is_object() || value.is_array())
                    })
                    .map(|(_, value)| value)
                    .collect();
                for value in members.into_iter().rev() {
                    stack.push((value, depth - 1));
                }
            }
            JsonValue::Array(items) => {
                for value in items.iter().rev().filter(|v| v.is_object() || v.is_array()) {
                    stack.push((value, depth - 1));
                }
            }
            _ => {}
        }
    }
    out
}
