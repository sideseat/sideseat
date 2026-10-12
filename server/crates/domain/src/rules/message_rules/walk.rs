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
            if let Some((rule, detail)) = std::iter::once(spec)
                .chain(&fragment_cases)
                .find_map(|case| traversal_defect(case).map(|detail| (case.id.clone(), detail)))
            {
                return Err(MessageCompileError::Inexpressible { rule, detail });
            }
            // A selection point with cases hands each candidate to them, and the case that answers builds the
            // value and chooses its target - so an envelope or a target on the selection point is never read.
            if !fragment_cases.is_empty() && (spec.wrap.is_some() || spec.emit.is_some()) {
                return Err(MessageCompileError::Inexpressible {
                    rule: spec.id.clone(),
                    detail: "a selection point with fragment or extra cases hands each candidate to them, \
                             and the case builds the value and chooses its target - so a `wrap` or `emit` \
                             here would be ignored; declare it on the cases",
                });
            }
            Ok(CompiledReading {
                spec: spec.clone(),
                fragment_cases,
            })
        })
        .collect()
}

/// Why a reading's traversal slots cannot do what they say: each of these compiled and did nothing, the same
/// defect as a construction branch accepting a sibling it returns before. Asked of a reading and of every case
/// inlined into it, since a case is a reading too.
///
/// | Declared | What happens |
/// | --- | --- |
/// | a lift `from: element` with no `descend` | the element *is* the candidate, so every member is already there |
/// | `else_element` with no `then_select` | it names the fallback for a coalesce that is not there |
/// | `parse` beside `descend` or `then_select` | `parse` admits only string elements, which have no members to descend into or select |
fn traversal_defect(spec: &Alternative) -> Option<&'static str> {
    if spec.descend.is_none()
        && spec
            .lift
            .iter()
            .any(|lift| lift.from == super::schema::LiftSource::Element)
    {
        return Some(
            "lifts from the element with no `descend`, where the element is already the value being emitted - \
             so every member is there and the lift copies nothing",
        );
    }
    if spec.else_element && spec.then_present_any_of.is_empty() {
        return Some(
            "declares `else_element` with no `then_select` - it names the fallback for a coalesce that is not there",
        );
    }
    if spec.parse.is_some() && (spec.descend.is_some() || !spec.then_present_any_of.is_empty()) {
        return Some(
            "parses its elements and also descends or selects inside them - `parse` admits only string \
             elements, which have no members, so the reading could never produce anything",
        );
    }
    None
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
    let mut out: Vec<(Reading, usize)> = Vec::new();
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
        // Capped as the single-node path is: what the walk gathers past the per-carrier ceiling is dropped at
        // emission anyway, so holding it would be memory spent on nothing. One over the ceiling, so the
        // emitter still sees the excess and reports it.
        let keep = (sideseat_core::constants::RULE_MAX_EMISSIONS_PER_CARRIER + 1)
            .saturating_sub(out.len());
        out.extend(
            here.built
                .into_iter()
                .zip(here.source_of.iter().copied().chain(std::iter::repeat(0)))
                .take(keep),
        );
        if depth == 0 || matched {
            continue;
        }
        // Scheduled work is bounded by the same ceiling that bounds visits: a node past it would be popped
        // only to trip the ceiling, so pushing it buys nothing and a payload of one node with millions of
        // children would hold them all on the stack. The first `room` children, in document order, are the
        // ones a bounded walk reaches; dropping the tail makes the scheduled total exactly the ceiling plus
        // one, so the warning above still fires on the last pop.
        let room = (sideseat_core::constants::RULE_WALK_MAX_NODES + 1)
            .saturating_sub(visited + stack.len());
        // The members a pruning clause read here, as a set: asked once per member below.
        let read_here: std::collections::HashSet<&str> = here
            .read_members
            .iter()
            .filter(|(clause, _)| walk.prune_read_by.contains(clause))
            .map(|(_, member)| member.as_str())
            .collect();
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
                        }) || read_here.contains(key.as_str());
                        !taken && (value.is_object() || value.is_array())
                    })
                    .map(|(_, value)| value)
                    .take(room)
                    .collect();
                for value in members.into_iter().rev() {
                    stack.push((value, depth - 1));
                }
            }
            JsonValue::Array(items) => {
                // An element a `prune_read_by` clause read here is skipped, by index, as a member is by name.
                let read = |at: usize| {
                    !read_here.is_empty() && read_here.contains(at.to_string().as_str())
                };
                let members: Vec<&JsonValue> = items
                    .iter()
                    .enumerate()
                    .filter(|(at, v)| (v.is_object() || v.is_array()) && !read(*at))
                    .map(|(_, v)| v)
                    .take(room)
                    .collect();
                for value in members.into_iter().rev() {
                    stack.push((value, depth - 1));
                }
            }
            _ => {}
        }
    }
    in_document_order(root, out)
}

/// Record the node member an element was, where its turn produced readings.
///
/// By identity, not by value: two members holding equal values are two members, and only the one this
/// element *is* was read. A member of an object by its name, an element of an array by its index: both are
/// the node's own children, and the walk visits both.
pub(super) fn note_read_member(
    node: &JsonValue,
    children: &mut Option<HashMap<*const JsonValue, String>>,
    pending: Option<(&JsonValue, usize)>,
    produced: usize,
    clause: &str,
    out: &mut Vec<(String, String)>,
) {
    let Some((element, start)) = pending else {
        return;
    };
    if produced <= start {
        return;
    }
    let index = children.get_or_insert_with(|| match node {
        JsonValue::Object(map) => map
            .iter()
            .map(|(key, child)| (child as *const JsonValue, key.clone()))
            .collect(),
        JsonValue::Array(items) => items
            .iter()
            .enumerate()
            .map(|(at, child)| (child as *const JsonValue, at.to_string()))
            .collect(),
        _ => HashMap::new(),
    });
    if let Some(member) = index.get(&(element as *const JsonValue)) {
        out.push((clause.to_string(), member.clone()));
    }
}

/// A walk's readings in the payload's document order, by the node each was built from.
///
/// The walk visits nodes in pre-order and a node's clauses read below it, so gathering order is not document
/// order: a message a node's clause selected from a later member came out ahead of one a descent found in an
/// earlier member, and nothing downstream can repair the order. So each reading's source node is located in
/// one pre-order pass over the payload - stopping once every source is placed - and the readings are sorted
/// stably by it: readings of one node keep their declaration order.
fn in_document_order(root: &JsonValue, readings: Vec<(Reading, usize)>) -> Vec<Reading> {
    let mut needed: std::collections::HashSet<usize> =
        readings.iter().map(|(_, source)| *source).collect();
    let mut position: HashMap<usize, usize> = HashMap::with_capacity(needed.len());
    let mut stack = vec![root];
    let mut next = 0usize;
    while let Some(node) = stack.pop() {
        if needed.is_empty() {
            break;
        }
        let address = std::ptr::from_ref(node) as usize;
        if needed.remove(&address) {
            position.insert(address, next);
        }
        next += 1;
        match node {
            JsonValue::Object(map) => stack.extend(map.values().rev()),
            JsonValue::Array(items) => stack.extend(items.iter().rev()),
            _ => {}
        }
    }
    let mut ordered = readings;
    // A source never found (none is expected: every source is a node of this payload) keeps its place last.
    ordered.sort_by_key(|(_, source)| position.get(source).copied().unwrap_or(usize::MAX));
    ordered.into_iter().map(|(reading, _)| reading).collect()
}
