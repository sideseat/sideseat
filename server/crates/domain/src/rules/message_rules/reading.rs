use super::*;

/// Parse a raw attribute value in the declared mode.
///
/// Each mode reproduces one thing the extractors do, and the difference between the first two is
/// load-bearing: `Json` *skips* a value it cannot parse, while `JsonOrString` keeps it as a string. An
/// extractor that used one where the other was meant would either drop a plain-text payload or store a
/// quoted fragment of JSON as prose.
pub(super) fn parse_value(raw: &str, mode: ParseMode) -> Option<JsonValue> {
    match mode {
        ParseMode::Json => serde_json::from_str(raw).ok(),
        // The elements are each serialised, because an OTLP array attribute cannot nest. **An array**, which
        // the mode names: a top-level object used to be returned unchanged, so a rule declaring this mode
        // silently accepted a shape its own declaration rules out.
        ParseMode::StringifiedArray => {
            let (value, unparsed) = sideseat_core::utils::json::parse_stringified_array_elements(
                serde_json::from_str(raw).ok()?,
            )?;
            if unparsed > 0 {
                // Kept, not dropped - dropping shortens the list silently and refusing the carrier loses the
                // elements that did parse - and reported, because a retained element is a producer defect that
                // used to be recorded nowhere at all.
                tracing::debug!(
                    target: "sideseat::rules",
                    unparsed,
                    "elements of a stringified array did not parse and are kept as strings"
                );
            }
            Some(value)
        }
        ParseMode::JsonOrString => Some(serde_json::from_str(raw).unwrap_or_else(|_| json!(raw))),
        ParseMode::PythonConstructorRepr => {
            crate::sideml::content::try_parse_python_constructor_repr(raw)
        }
        // Prose. Parsing it would turn a bare word into a non-string and an accidental digit string
        // into a number.
        ParseMode::Text => Some(json!(raw)),
    }
}

/// Everything one payload yields: the first-winning alternatives, every cumulative reading, and a
/// fallback used only when neither produced anything.
///
/// The distinction between "first wins" and "all contribute" is not stylistic. One dialect writes a turn's
/// history under one member and the answer itself under another, so reading them as alternatives dropped
/// the assistant output of every run that carried history.
pub(super) fn all_readings(
    parsed: &JsonValue,
    rule: &CompiledMessageRule,
    build: Option<Construction<'_>>,
) -> Selection {
    let mut out = Selection {
        built: Vec::new(),
        recognised: Vec::new(),
    };
    if !rule.alternatives.is_empty() {
        out.absorb(readings(parsed, &rule.alternatives, build));
    }
    for alternative in &rule.also {
        out.absorb(readings(parsed, std::slice::from_ref(alternative), build));
    }
    if out.built.is_empty() {
        if rule.alternatives.is_empty() && rule.also.is_empty() && rule.fallback.is_empty() {
            // No readings declared at all: the payload is the observation - still built, since the rule's own
            // envelope applies to it.
            return Selection {
                built: match built(parsed.clone(), None, build) {
                    Some(value) => vec![(value, None, Vec::new())],
                    None => Vec::new(),
                },
                recognised: Vec::new(),
            };
        }
        for alternative in &rule.fallback {
            out.absorb(readings(parsed, std::slice::from_ref(alternative), build));
        }
    }
    out
}

/// Query a value with a compiled JSONPath.
///
/// One definition, so every path in the vocabulary is the same language. It replaced a hand-rolled resolver
/// whose dotted-step syntax could not distinguish a member *named* `event.name` from a nested `event` ->
/// `name` - a real defect, and the sort of thing a standard defines away (`$['event.name']`).
///
/// The nodes are **borrowed from the value queried**, which is the property the whole choice rests on:
/// cloning one out keeps the provider's member order. See
/// `the_selection_language_behaves_as_the_engine_assumes`.
/// One match of a path used in a **singular** role, reporting when the payload offered more than one.
///
/// Fifteen sites take `query(...).into_iter().next()`, and a JSONPath is plural by nature: `$.*` matches every
/// member, so `elements.select: "$.*"` reads the *first* array of several and the others are gone with nothing
/// said. The same silent-first rule reaches a grouped `collect`, a `tag_from`, the indexed projections and
/// several constructors.
///
/// Behaviour is unchanged deliberately: which of those sites a real payload makes ambiguous is not something to
/// guess at, and a strict "at most one" refusal applied blind would reject shapes the corpus may depend on. What
/// this changes is that the ambiguity is **reported**, which turns the question into a measurement. An author who
/// means the first can say `[0]`; the strict roles come after there is evidence about which sites need them.
pub(super) fn singular<'v>(
    value: &'v JsonValue,
    path: &serde_json_path::JsonPath,
    role: &str,
) -> Option<&'v JsonValue> {
    let matched = query(value, path);
    if matched.len() > 1 {
        tracing::debug!(
            target: "sideseat::rules",
            role,
            matches = matched.len(),
            "a path used in a singular role matched more than once; the first is taken"
        );
    }
    matched.into_iter().next()
}

pub(in crate::rules) fn query<'v>(
    value: &'v JsonValue,
    path: &serde_json_path::JsonPath,
) -> Vec<&'v JsonValue> {
    path.query(value).into_iter().collect()
}

/// The observations one payload yields, under the first alternative that produces any.
///
/// An ordered coalesce over documented shapes. With no alternatives the payload is emitted as it stands,
/// which is what a carrier holding exactly one message needs. "The first that produces any" is the whole
/// control flow, and it is deliberately all there is: a shape that yields nothing is not an error, it is
/// evidence the payload is in a different one of its documented forms.
pub(super) fn readings(
    parsed: &JsonValue,
    alternatives: &[CompiledReading],
    build: Option<Construction<'_>>,
) -> Selection {
    if alternatives.is_empty() {
        return Selection {
            built: match built(parsed.clone(), None, build) {
                Some(value) => vec![(value, None, Vec::new())],
                None => Vec::new(),
            },
            recognised: Vec::new(),
        };
    }
    // Accumulated across **every** alternative, not per alternative: a clause that recognised the node and
    // could not build says so whether or not a later clause then answered.
    let mut recognised: Vec<String> = Vec::new();
    for reading in alternatives {
        let alternative = &reading.spec;
        // Asked of the enclosing value, before anything is selected out of it: the discriminator for a
        // batch of results is the type of the message holding them.
        if !predicates_hold(parsed, &alternative.require_parent) {
            continue;
        }
        let selected: Vec<&JsonValue> = match &alternative.select {
            Some(path) => query(parsed, path),
            None => vec![parsed],
        };
        if selected.is_empty() {
            continue;
        }
        let elements: Vec<&JsonValue> = if alternative.each {
            let mut items = Vec::new();
            let mut all_arrays = true;
            for value in &selected {
                match value.as_array() {
                    Some(inner) => items.extend(inner.iter()),
                    // Declared as a list and is not one: this is not the shape, so try the next.
                    None => all_arrays = false,
                }
            }
            if !all_arrays {
                continue;
            }
            items
        } else {
            selected
        };

        let mut produced = Vec::new();
        for element in elements {
            // For each element, the first of these sub-paths that resolves - decided per element, because
            // one dialect's groups each either wrap their contents under one of two spellings or are the
            // content themselves, and choosing once for the whole array would drop the odd one out.
            let element: Vec<&JsonValue> = if !alternative.then_present_any_of.is_empty() {
                // The first path that resolves *at all*. A present-but-empty member has declared nothing,
                // and yields nothing - it does not fall through to the element itself.
                // **Three answers, not two.** A wrapper *is* a list of declarations, so a member that is
                // present and not one has not declared its contents - and treating that as the member being
                // *absent* sent it to the element fallback, which emits the whole wrapper as a tool
                // definition. `{"function_declarations": {"name": "weather"}}` became a tool that way.
                //
                // Presence also **chooses the representation**: once a path names something, a later spelling
                // is not tried, because falling through would answer from a representation the producer did
                // not use.
                let found = alternative
                    .then_present_any_of
                    .iter()
                    .find_map(|path| singular(element, path, "then_present_any_of"));
                let fallback = |which: Option<super::schema::PresenceFallback>| match which
                    .unwrap_or(if alternative.else_element {
                        super::schema::PresenceFallback::Element
                    } else {
                        super::schema::PresenceFallback::Nothing
                    }) {
                    super::schema::PresenceFallback::Element => Some(vec![element]),
                    super::schema::PresenceFallback::Nothing => None,
                };
                match found {
                    // Present and a list: its members are the contents, an empty one included - a producer
                    // writing `[]` has declared no tools, which is a statement.
                    Some(value) if value.is_array() => value
                        .as_array()
                        .map_or_else(Vec::new, |items| items.iter().collect()),
                    // Present and something else. Recovered rather than refused, because the enclosing object
                    // independently describes a valid reading - and **reported**, because the member the
                    // producer wrote is unusable and that used to be recorded nowhere.
                    Some(value) => {
                        let refusal = super::refusal::Refusal::new(
                            super::expr::ClausePath::root(alternative.id.clone()),
                            "then_present_any_of",
                            super::refusal::Unusable::WrongMember {
                                detail: format!(
                                    "a wrapper member is a list of declarations; found {}",
                                    match value {
                                        JsonValue::Object(_) => "an object",
                                        JsonValue::String(_) => "a string",
                                        JsonValue::Number(_) => "a number",
                                        JsonValue::Bool(_) => "a boolean",
                                        JsonValue::Null => "null",
                                        JsonValue::Array(_) => "an array",
                                    }
                                ),
                            },
                        );
                        tracing::debug!(
                            target: "sideseat::rules",
                            clause = %refusal.clause,
                            carrier = %refusal.carrier,
                            cause = %refusal.cause,
                            "a presence coalesce named a member of the wrong shape"
                        );
                        match fallback(alternative.on_malformed) {
                            Some(recovered) => recovered,
                            None => continue,
                        }
                    }
                    // Absent: nothing named anything, which is what a fallback is for.
                    None => match fallback(alternative.on_absent) {
                        Some(recovered) => recovered,
                        None => continue,
                    },
                }
            } else if alternative.then_any_of.is_empty() {
                vec![element]
            } else {
                let found = alternative
                    .then_any_of
                    .iter()
                    .map(|path| query(element, path))
                    .find(|found| !found.is_empty());
                match found {
                    Some(found) => found,
                    None if alternative.else_element => vec![element],
                    None => continue,
                }
            };
            for element in element {
                // Descend where declared, then lift - **one** copy step whose conflict policy is declared,
                // where there were two members with opposite unstated ones.
                let mut candidate = match &alternative.descend {
                    Some(member) => {
                        let Some(inner) = element.get(member.as_str()) else {
                            continue;
                        };
                        inner.clone()
                    }
                    None => element.clone(),
                };
                if !alternative.lift.is_empty()
                    && let Some(object) = candidate.as_object_mut()
                {
                    for lift in &alternative.lift {
                        // `element` is the value the selection landed on, `parent` the value it came out of.
                        let source = match lift.from {
                            super::schema::LiftSource::Element => element,
                            super::schema::LiftSource::Parent => parsed,
                        };
                        for member in &lift.members {
                            if object.contains_key(member.as_str())
                                && lift.on_conflict == super::schema::LiftConflict::KeepTarget
                            {
                                continue;
                            }
                            if let Some(value) = source.get(member.as_str()) {
                                object.insert(member.clone(), value.clone());
                            }
                        }
                    }
                }
                let candidate = candidate;
                // Trim declared per reading, because trimming a payload meant to be verbatim would change it.
                let candidate = match (alternative.trim, candidate.as_str()) {
                    (true, Some(text)) => json!(text.trim()),
                    _ => candidate,
                };
                if !predicates_hold(&candidate, &alternative.require) {
                    continue;
                }
                // The fragment decides what the element *is*; this reading decided where to look. Splitting
                // them is why one dialect can recognise its message shapes at four selection points with one
                // table.
                if !reading.fragment_cases.is_empty() {
                    let cases: Vec<CompiledReading> = reading
                        .fragment_cases
                        .iter()
                        .map(|spec| CompiledReading {
                            spec: spec.clone(),
                            fragment_cases: Vec::new(),
                        })
                        .collect();
                    // The selection point and then the case that answered: the fragment decides what the
                    // element *is*, this reading decided where to look, and a diagnostic needs both. Nested
                    // fragment cases lost the selection-point id entirely before this.
                    let inner = readings(&candidate, &cases, build);
                    // A fragment case matching means the *selection point* recognised this node, which is what
                    // a walk stop is about - the case says what the value is, the selection point says the
                    // rule's clause found it here.
                    if !inner.recognised.is_empty() && !recognised.contains(&alternative.id) {
                        recognised.push(alternative.id.clone());
                    }
                    produced.extend(inner.built.into_iter().map(|(value, target, mut steps)| {
                        let mut path = vec![alternative.id.clone()];
                        path.append(&mut steps);
                        (value, target, path)
                    }));
                    continue;
                }
                // Recognition is recorded **before** construction: the candidate passed every predicate this
                // clause states, so the payload is this shape whatever happens to the envelope.
                if !recognised.contains(&alternative.id) {
                    recognised.push(alternative.id.clone());
                }
                // **Built here**, so a candidate whose envelope cannot be made counts as this alternative
                // producing nothing - and the coalesce moves on to the next shape and then to the fallback.
                let Some(value) = built(candidate, alternative.wrap.as_ref(), build) else {
                    continue;
                };
                produced.push((value, alternative.emit, vec![alternative.id.clone()]));
            }
        }
        if !produced.is_empty() {
            return Selection {
                built: produced,
                recognised,
            };
        }
    }
    // Recognised but unbuildable: nothing was produced, and the recognition still travels out - a walk must not
    // descend into a node a clause identified as a message merely because its envelope failed.
    Selection {
        built: Vec::new(),
        recognised,
    }
}

/// One emission's evidence: the rule, plus the clauses inside it that produced this observation.
///
/// A single witness for the ordinary case. Where several clauses contributed - an aggregate's readings, a
/// grouped run's cases - the caller passes them all and each becomes its own path, because "these three
/// declarations produced this" is the true statement and picking one of them is not.
pub(super) fn rule_evidence(
    rule: &CompiledMessageRule,
    paths: &[Vec<String>],
) -> super::expr::EvidenceSet {
    let root = || super::expr::ClausePath::root(rule.rule_id.clone());
    // Each element is a **path**, not a step: a grouped element run's witnesses share the pass and differ in
    // the case, so `rule -> pass -> case` twice is the true statement while `rule -> pass` beside `rule -> case`
    // is two paths neither of which exists.
    let witnesses: Vec<super::expr::ClausePath> = paths
        .iter()
        .map(|steps| {
            steps
                .iter()
                .fold(root(), |path, step| path.then(step.clone()))
        })
        .collect();
    super::expr::EvidenceSet::of(witnesses).unwrap_or_else(|| super::expr::EvidenceSet::one(root()))
}

/// A reading's value once its envelope is applied, or `None` when the envelope cannot be built.
///
/// A reading's own envelope wins over the rule's, which is what `or` says. With no construction context - the
/// aggregate path - the value is returned as it stands, and compilation refuses a per-reading envelope there so
/// nothing can be silently dropped.
pub(super) fn built(
    value: JsonValue,
    reading_wrap: Option<&WrapSpec>,
    build: Option<Construction<'_>>,
) -> Option<JsonValue> {
    let Some(build) = build else {
        return Some(value);
    };
    match reading_wrap.or(build.rule_wrap) {
        Some(wrap) => wrapped(value, wrap, build.ctx, Some(build.root)),
        None => Some(value),
    }
}

/// One entry per index of a dotted attribute family, assembled from the keys under it.
///
/// The convention flattens a list of objects into `<prefix>.<index>.<member>`, so this is the inverse:
/// gather every key of an index, strip the prefix, and keep the remainder as the member name - nested
/// members included, so `content.0.text` stays `content.0.text` rather than being lost or guessed at.
///
/// Ordered by index, because a `BTreeMap` key is the number: reading the attribute map directly would
/// order turns by hash.
pub(super) fn indexed_entries(
    attrs: &HashMap<String, String>,
    family: &str,
    read: &ReadSpec,
    require: Option<&MemberRequirements>,
) -> Vec<IndexedEntry> {
    // Every other parameter was a facet of the same `ReadSpec`, and threading them one by one meant a new
    // facet was a new argument at every call site.
    let entry_member = read.entry_member.as_deref();
    let numeric = &read.numeric_members;
    let overlay = read.overlay.as_ref();
    let entry_value = read.entry_value.as_ref();
    let entry_value_parse = read.entry_value_parse;
    // Parsed once for the whole family: the counterpart list describes every entry, so parsing it per
    // entry would re-parse one payload as many times as there are messages.
    let counterparts = overlay.and_then(|overlay| counterpart_list(attrs, overlay));
    // **Bucketed in one pass**, keyed by index and holding the member name after `family.<index>.`. The
    // discovery pass and then a full scan of the attribute map *per index* is quadratic in the family's size:
    // a hundred-turn conversation flattened into a family means a hundred walks over every attribute the span
    // carries, twice, plus one per `require`d member. A `BTreeMap` because the entries are read in index order,
    // which is what keeps a turn sequence from being a hash order.
    let family_dot = format!("{family}.");
    let mut buckets: std::collections::BTreeMap<usize, Vec<(&str, &String)>> =
        std::collections::BTreeMap::new();
    for (key, value) in attrs {
        if let Some(rest) = key.strip_prefix(&family_dot)
            && let Some(index) = rest.split('.').next()
            && let Ok(parsed) = index.parse::<usize>()
        {
            // The remainder past `family.<index>` - empty where the key *is* the index, which a producer can
            // write and which belongs to no member.
            let member = rest[index.len()..].strip_prefix('.').unwrap_or("");
            buckets.entry(parsed).or_default().push((member, value));
        }
    }
    // Sorted once per bucket rather than once per read of it: the attribute map's order is randomised per
    // process and these objects are persisted with their insertion order.
    for members in buckets.values_mut() {
        members.sort_unstable_by_key(|(member, _)| *member);
    }

    let mut out: Vec<IndexedEntry> = Vec::new();
    for (index, members) in &buckets {
        let index = *index;
        // The physical keys this entry read. An entry's *tag* is `family.N`, which is a name for the entry
        // and not a key any producer wrote - so owning the tag left `family.N.content` free for another
        // rule, and the overlay's own attribute free for the dialect that reads it as a whole payload.
        let mut consumed: Vec<String> = Vec::new();
        let entry_prefix = format!("{family}.{index}");
        // Where the message sits: the entry itself, or a sub-level of it.
        let subject_prefix = match entry_member {
            Some(member) => format!("{entry_prefix}.{member}"),
            None => entry_prefix.clone(),
        };

        // An index exists as soon as any key mentions it, and a family holds keys that are not messages.
        // Asked of this index's bucket, so a requirement costs the bucket rather than the whole span.
        if let Some(require) = require
            && !bucket_members_present(members, entry_member, require)
        {
            continue;
        }

        let mut object = serde_json::Map::new();
        // The subject's own members, unprefixed. From this index's bucket, already sorted.
        let subject_dot = format!("{subject_prefix}.");
        let within = match entry_member {
            Some(nested) => format!("{nested}."),
            None => String::new(),
        };
        for (member, value) in members
            .iter()
            .filter_map(|(member, value)| match entry_member {
                Some(_) => member.strip_prefix(within.as_str()).map(|m| (m, *value)),
                None => Some((*member, *value)),
            })
            .filter(|(member, _)| !member.is_empty())
        {
            consumed.push(format!("{subject_dot}{member}"));
            object.insert(member.to_string(), member_value(member, value, numeric));
        }
        // Where the message is nested, the entry's *other* members come too: they belong to the same
        // observation, and the sub-level's own keys are already in, so they are skipped here.
        if entry_member.is_some() {
            let entry_dot = format!("{entry_prefix}.");
            for (member, value) in members
                .iter()
                .filter(|(member, _)| !member.is_empty() && !member.starts_with(within.as_str()))
            {
                consumed.push(format!("{entry_dot}{member}"));
                object.insert(member.to_string(), member_value(member, value, numeric));
            }
        }
        if let Some(overlay) = overlay
            && let Some(content) = counterpart_content(overlay, counterparts.as_ref(), index)
            && object
                .keys()
                .any(|member| member.starts_with(overlay.when_member_prefix.as_str()))
        {
            let flattened: Vec<String> = object
                .keys()
                .filter(|member| member.starts_with(overlay.when_member_prefix.as_str()))
                .cloned()
                .collect();
            for member in flattened {
                object.remove(&member);
            }
            // The overlay's payload was read, so this entry owns it: a dialect joining its flattened family
            // against a serialised copy has consumed that copy, and leaving it unclaimed let the dialect
            // that reads it whole emit the same turn again.
            consumed.push(overlay.from.clone());
            object.insert(overlay.as_member.clone(), content);
        }
        // A projection reads one value out of the entry: the entry is a wrapper around a single payload,
        // and the payload is the datum. An entry the projection does not find contributes nothing - it is
        // not this shape - rather than contributing the wrapper.
        match entry_value {
            Some(path) => {
                let assembled = JsonValue::Object(object);
                if let Some(found) = singular(&assembled, path, "compose require_after") {
                    // A declared parse mode decides what a malformed payload means. Without it the member
                    // has already been sniffed to a string, and emitting that string as a tool definition
                    // reports junk where the retired code reported nothing.
                    let value = match (entry_value_parse, found.as_str()) {
                        (Some(mode), Some(text)) => parse_value(text, mode),
                        (Some(_), None) => Some(found.clone()),
                        (None, _) => Some(found.clone()),
                    };
                    if let Some(value) = value {
                        out.push(IndexedEntry {
                            carrier: subject_prefix,
                            value,
                            consumed,
                        });
                    }
                }
            }
            None => out.push(IndexedEntry {
                carrier: subject_prefix,
                value: JsonValue::Object(object),
                consumed,
            }),
        }
    }
    out
}

/// One entry of an indexed family: its tag, its assembled payload, and the physical keys it read.
///
/// The keys are separate from the tag because they are different kinds of name. `llm.input_messages.0.message`
/// is assembled here to identify the entry; `llm.input_messages.0.message.role` is a key a producer wrote,
/// and only the second is something another rule could also read.
pub(super) struct IndexedEntry {
    pub(super) carrier: String,
    pub(super) value: JsonValue,
    pub(super) consumed: Vec<String>,
}

/// The counterpart list a positional overlay joins against, where the span carries one this dialect wrote.
pub(super) fn counterpart_list(
    attrs: &HashMap<String, String>,
    overlay: &OverlaySpec,
) -> Option<Vec<JsonValue>> {
    let raw = attrs.get(overlay.from.as_str())?;
    let parsed = parse_value(raw, overlay.parse.unwrap_or(ParseMode::Json))?;
    let list = overlay
        .select_any_of
        .iter()
        .find_map(|path| singular(&parsed, path, "overlay select_any_of")?.as_array())?;
    // A batch of exactly one conversation: its single member is the list of messages. Not a mixed or
    // longer list - two batches are two conversations, and the witness below decides whether whatever is
    // left is this dialect's own serialisation.
    let list = match (overlay.unwrap_single_element_list, list.as_slice()) {
        (true, [only]) => only.as_array().unwrap_or(list),
        _ => list,
    };
    if list.is_empty() {
        return None;
    }
    let witness = JsonValue::Array(list.clone());
    if !predicates_hold(&witness, &overlay.witness) {
        return None;
    }
    Some(list.clone())
}

/// The content the counterpart at this position holds, where it is an improvement on the flattened form.
pub(super) fn counterpart_content(
    overlay: &OverlaySpec,
    counterparts: Option<&Vec<JsonValue>>,
    index: usize,
) -> Option<JsonValue> {
    let found = overlay
        .content_any_of
        .iter()
        .find_map(|path| singular(counterparts?.get(index)?, path, "overlay content_any_of"))?;
    predicates_hold(found, &overlay.require).then(|| found.clone())
}

/// A named member read as a number where its text is one, otherwise the ordinary sniff.
pub(super) fn member_value(member: &str, raw: &str, numeric: &[String]) -> JsonValue {
    if numeric.iter().any(|name| name == member)
        && let Ok(number) = raw.parse::<f64>()
        && let Some(number) = serde_json::Number::from_f64(number)
    {
        return JsonValue::Number(number);
    }
    sniffed_value(raw)
}

/// A member of an indexed family: JSON where it looks like JSON, the text otherwise.
///
/// The sniff matters. Parsing everything would turn `true`, `42` and a bare word into non-strings, and
/// parsing nothing would store an object as prose - so the test is whether the value *opens* as JSON,
/// with the raw text kept when it opens that way and does not parse.
pub(super) fn sniffed_value(raw: &str) -> JsonValue {
    if raw.starts_with('{') || raw.starts_with('[') {
        serde_json::from_str(raw).unwrap_or_else(|_| json!(raw))
    } else {
        json!(raw)
    }
}

/// Both gates, in one place so every read form is subject to them.
pub(super) fn gates_allow(rule: &CompiledMessageRule, ctx: &MessageContext<'_>) -> bool {
    if let Some(scope) = &rule.instrumentation_scope {
        if ctx.scope_name != Some(scope.name.as_str()) {
            return false;
        }
        if let Some(prefix) = &scope.version_prefix
            && !ctx
                .scope_version
                .is_some_and(|version| version.starts_with(prefix))
        {
            return false;
        }
    }
    if let Some(gate) = &rule.when
        && !super::detect_rules::compiled_signals_hold(gate, ctx.span_name, ctx.gate_attrs)
    {
        return false;
    }
    if let Some(gate) = &rule.unless
        && super::detect_rules::compiled_signals_hold(gate, ctx.span_name, ctx.gate_attrs)
    {
        return false;
    }
    true
}

/// The attribute a rule reads and its raw value: the named one, or the first of its alternatives the
/// span carries.
///
/// Returns the key *found*, not the key asked for, because that key becomes the carrier tag and two
/// spellings of one payload must stay distinguishable.
/// Every carrier this rule names that the span carries, in declared order.
///
/// Not the first, unlike an ordinary read: a framework may write the same tools under several keys at
/// different richness, and each is its own observation - so all of them are read and the best copy per
/// name wins downstream, rather than the richest being hidden behind whichever key was declared first.
pub(super) fn carrier_texts<'p, 's>(
    rule: &'p CompiledMessageRule,
    ctx: &MessageContext<'s>,
) -> Vec<(&'p str, &'s str)> {
    rule.read
        .attribute
        .as_deref()
        .into_iter()
        .chain(rule.read.each.iter().map(String::as_str))
        .filter_map(|key| ctx.span_attrs.get(key).map(|raw| (key, raw.as_str())))
        .collect()
}

pub(super) fn resolve_attribute<'p, 's>(
    read: &'p ReadSpec,
    attrs: &'s HashMap<String, String>,
) -> Option<(&'p str, &'s str)> {
    if let Some(attribute) = read.attribute.as_deref() {
        return attrs.get(attribute).map(|raw| (attribute, raw.as_str()));
    }
    read.first_present
        .iter()
        .find_map(|key| attrs.get(key).map(|raw| (key.as_str(), raw.as_str())))
}
