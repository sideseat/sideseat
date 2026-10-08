use super::*;

/// Build the declared envelope around a read value.
pub(super) fn wrapped(
    value: JsonValue,
    wrap: &WrapSpec,
    ctx: &MessageContext<'_>,
    payload: Option<&JsonValue>,
) -> Option<JsonValue> {
    // The reading as it arrived, before a content member was selected out of it: an attachment may name a
    // sibling of the content, which is gone once the content replaces the value.
    let subject = value.clone();
    let subject = Some(&subject);
    // A content block, when the carrier holds one part of a block rather than a whole message.
    // The role, and the part of the reading that is the content, may both come from the payload.
    let role = wrap
        .role_path()
        .and_then(|path| singular(&value, path, "wrap role_from"))
        .and_then(JsonValue::as_str)
        .and_then(|found| match wrap.role_map() {
            None => Some(found.to_string()),
            Some((table, closed)) => match table.get(found) {
                Some(mapped) => mapped.as_str().map(str::to_string),
                // Closed: the member names a speaker rather than a role, so an unlisted value is not one.
                None if closed => None,
                None => Some(found.to_string()),
            },
        })
        .or_else(|| wrap.role.clone());
    let content_paths: Vec<&serde_json_path::JsonPath> = wrap.content_from_any_of.iter().collect();
    let value = if content_paths.is_empty() {
        value
    } else {
        // The first path that resolves: one dialect serialises a message three ways and the content sits in a
        // different member each time.
        match content_paths
            .iter()
            .find_map(|path| singular(&value, path, "wrap content_from_any_of"))
        {
            Some(found) => found.clone(),
            None => match &wrap.content_default {
                Some(default) => default.clone(),
                None => return None,
            },
        }
    };
    // Wrap only where the value is not already message-shaped: a generic carrier holds either.
    if wrap.only_plain_data && !crate::sideml::is_plain_data_or_content(&value) {
        return Some(value);
    }
    let value = match &wrap.block {
        Some(block) => JsonValue::Array(vec![built_block(value, block, ctx, payload, subject)]),
        None => value,
    };
    // A block built from another member, placed before the content: one dialect reports a model's reasoning
    // beside its reply, and the canonical form is a thinking block ahead of the text.
    let value = match &wrap.prepend_block {
        Some(spec) => {
            match subject
                .and_then(|s| singular(s, &spec.from, "block from"))
                .filter(|found| predicates_hold(found, &spec.require))
            {
                Some(found) => {
                    let prefix = built_block(found.clone(), &spec.block, ctx, payload, subject);
                    match value {
                        JsonValue::Array(items) => {
                            let mut all = vec![prefix];
                            all.extend(items);
                            JsonValue::Array(all)
                        }
                        other => JsonValue::Array(vec![prefix, other]),
                    }
                }
                None => value,
            }
        }
        None => value,
    };
    let mut object = serde_json::Map::new();
    if let Some(role) = role {
        object.insert("role".to_string(), json!(role));
    }
    for (member, literal) in &wrap.members {
        object.insert(member.clone(), literal.clone());
    }
    let content_member = wrap.content_as.as_deref().unwrap_or("content");
    // Before the content member, then the content, then after - because the order is observable: see
    // `AttachSpec::after_content`.
    for attach in wrap.attach.iter().filter(|a| !a.after_content) {
        if let Some(attached) = attached_value(attach, ctx, payload, subject) {
            object.insert(attach.as_member.clone(), attached);
        }
    }
    // The canonical tool-call list replaces the content: a message that carries calls carries no text.
    match &wrap.tool_calls_from {
        Some(spec) => {
            object.insert(
                "tool_calls".to_string(),
                JsonValue::Array(canonical_tool_calls(subject, spec)?),
            );
        }
        None => {
            object.insert(content_member.to_string(), value);
        }
    }
    if let Some(spec) = &wrap.tool_call_from
        && let Some(call) = single_tool_call(subject, spec)
    {
        object.insert("tool_call".to_string(), call);
    }
    for attach in wrap.attach.iter().filter(|a| a.after_content) {
        if let Some(attached) = attached_value(attach, ctx, payload, subject) {
            object.insert(attach.as_member.clone(), attached);
        }
    }
    Some(JsonValue::Object(object))
}

/// One tool call as `{name, arguments}`, the convention `sideml/tools.rs` unwraps.
pub(super) fn single_tool_call(
    subject: Option<&JsonValue>,
    spec: &SingleToolCallSpec,
) -> Option<JsonValue> {
    let subject = subject?;
    // Only a string names a tool. A number or an object here is not a name, and the declared default is
    // what the retired path used - reporting the structure as a name builds an unusable canonical call.
    let name = query(subject, &spec.name)
        .into_iter()
        .next()
        .filter(|found| found.is_string())
        .cloned()
        .or_else(|| spec.name_default.clone())?;
    let arguments = query(subject, &spec.arguments)
        .into_iter()
        .next()
        .cloned()
        .or_else(|| spec.arguments_default.clone())
        .unwrap_or(json!({}));
    Some(json!({"name": name, "arguments": arguments}))
}

/// The canonical tool-call list: `{id, type: "function", function: {name, arguments}}` per call.
///
/// A call with no id or no name is dropped - the id is what pairs a result with its call, and a nameless
/// call names nothing to run. Arguments arrive as a serialised JSON string as often as an object, and are
/// parsed here so nothing downstream has to know that one member is encoded twice.
pub(super) fn canonical_tool_calls(
    subject: Option<&JsonValue>,
    spec: &ToolCallsSpec,
) -> Option<Vec<JsonValue>> {
    let Some(subject) = subject else {
        return Some(Vec::new());
    };
    let mut calls = Vec::new();
    let mut invalid = 0;
    for call in query(subject, &spec.select) {
        let built = (|| {
            let id = singular(call, &spec.id, "tool call id")?.as_str()?;
            let name = singular(call, &spec.name, "tool call name")?.as_str()?;
            let arguments = match singular(call, &spec.arguments, "tool call arguments") {
                Some(found) => match found.as_str() {
                    Some(text) => serde_json::from_str(text).unwrap_or(json!(text)),
                    None => found.clone(),
                },
                None => json!({}),
            };
            Some(json!({
                "id": id,
                "type": "function",
                "function": {"name": name, "arguments": arguments},
            }))
        })();
        match built {
            Some(call) => calls.push(call),
            None => invalid += 1,
        }
    }
    // **Reported under either policy.** A call with no id or no name is a producer defect, and the loop that
    // read it silently absorbing that is how a response that called two tools came to show one.
    if invalid > 0 {
        tracing::debug!(
            target: "sideseat::rules",
            invalid,
            policy = ?spec.on_invalid_item,
            "tool calls in a list could not be built - they carry no id or no name"
        );
        if spec.on_invalid_item == super::schema::InvalidItem::FailMessage {
            // The construction is malformed, so the coalesce moves on and the rule's `fallback` gets its turn -
            // which is the difference between "the message is incomplete" and "this shape did not apply".
            return None;
        }
    }
    Some(calls)
}

/// The block a rule builds around its read value.
pub(super) fn built_block(
    value: JsonValue,
    block: &BlockSpec,
    ctx: &MessageContext<'_>,
    payload: Option<&JsonValue>,
    subject: Option<&JsonValue>,
) -> JsonValue {
    let mut object = serde_json::Map::new();
    object.insert("type".to_string(), json!(block.block_type));
    for attach in block.attach.iter().filter(|a| !a.after_content) {
        if let Some(attached) = attached_value(attach, ctx, payload, subject) {
            object.insert(attach.as_member.clone(), attached);
        }
    }
    object.insert(
        block.content_as.as_deref().unwrap_or("content").to_string(),
        value,
    );
    for attach in block.attach.iter().filter(|a| a.after_content) {
        if let Some(attached) = attached_value(attach, ctx, payload, subject) {
            object.insert(attach.as_member.clone(), attached);
        }
    }
    JsonValue::Object(object)
}

/// One attachment's value, or `None` where nothing supplied one.
///
/// One order for every source: a value is **read** - from the first of `from_value`, `from_path`, `from` and the
/// span name that supplies one, through that source's steps - then `where` is asked of it, and then a literal
/// (`value`, or a closed `map`'s) replaces it, for a flag. Where nothing was read, the `default` is attached.
pub(super) fn attached_value(
    attach: &AttachSpec,
    ctx: &MessageContext<'_>,
    payload: Option<&JsonValue>,
    subject: Option<&JsonValue>,
) -> Option<JsonValue> {
    // A literal on its own, naming no source: the member is part of the shape rather than something read.
    // A content block's unsigned `signature` is exactly this, and its value is `null`.
    if attach.from.is_none()
        && attach.from_value_any_of.is_empty()
        && attach.from_path.is_none()
        && attach.or_span_name_after().is_none()
    {
        return attach.value.clone();
    }
    match read_attachment(attach, ctx, payload, subject) {
        Attachment::Read { value, literal } => {
            if !predicates_hold(&value, &attach.require) {
                return None;
            }
            // A flag: the literal is the point, not the value that proved it.
            Some(literal.or_else(|| attach.value.clone()).unwrap_or(value))
        }
        Attachment::Unusable => None,
        Attachment::Nothing => attach.default.clone(),
    }
}

/// What an attachment's sources supplied.
enum Attachment {
    /// A value, and the literal a closed `map` attaches in its place.
    Read {
        value: JsonValue,
        literal: Option<JsonValue>,
    },
    /// A source supplied something that cannot be attached, and the member is left off.
    Unusable,
    /// No source supplied anything: the `default` applies.
    Nothing,
}

/// The value an attachment's first supplying source reads, through that source's own steps.
fn read_attachment(
    attach: &AttachSpec,
    ctx: &MessageContext<'_>,
    payload: Option<&JsonValue>,
    subject: Option<&JsonValue>,
) -> Attachment {
    let read = |value: JsonValue| Attachment::Read {
        value,
        literal: None,
    };
    // A member holding serialised JSON is parsed where declared, because leaving it a string means whoever reads
    // it later has to know that this one member is encoded twice. One reading for both paths into a value.
    let decoded = |found: &JsonValue| match (attach.parse, found.as_str()) {
        (Some(mode), Some(text)) => parse_value(text, mode),
        _ => Some(found.clone()),
    };
    let folded = |value: JsonValue| match (attach.lowercase(), value.as_str()) {
        (true, Some(text)) => json!(text.to_lowercase()),
        _ => value,
    };
    // Ordered paths into the value being wrapped, for a member that may sit at the top level or under the
    // wrapper a serialiser added. Nothing there falls through to the payload path, which is how "the element's
    // own, else its parent's" is one member rather than two that overwrite each other.
    if let Some(found) = subject.and_then(|subject| {
        attach
            .from_value_any_of
            .iter()
            .find_map(|path| singular(subject, path, "attach from_value_any_of"))
    }) {
        return decoded(found).map_or(Attachment::Unusable, read);
    }
    // A member of the rule's own payload, where the dialect reports it beside the content rather than inside it.
    // Falls through like a value path: one member, two source forms, one answer to "nothing here".
    if let Some(found) = attach
        .from_path
        .as_ref()
        .zip(payload)
        .and_then(|(path, payload)| singular(payload, path, "attach from_path"))
    {
        return decoded(found).map_or(Attachment::Unusable, |value| read(folded(value)));
    }
    if let Some(raw) = attach
        .from
        .as_ref()
        .and_then(|key| ctx.span_attrs.get(key))
        .filter(|raw| !(attach.blank_is_absent() && raw.trim().is_empty()))
    {
        let raw = if attach.strip_bracket_tag() {
            split_bracket_tag(raw).1
        } else {
            raw.as_str()
        };
        if let Some((expected, literal)) = attach.when_equals() {
            return if raw == expected {
                Attachment::Read {
                    value: json!(raw),
                    literal: Some(literal.clone()),
                }
            } else {
                Attachment::Unusable
            };
        }
        // A value that will not parse, or a selection that names nothing, falls through to the span name and
        // the default, which is what an unparseable structured member should do: the member exists in the
        // shape, so it carries its empty form.
        let parsed = parse_value(raw, attach.parse.unwrap_or(ParseMode::Text));
        let parsed = match &attach.select {
            Some(path) => parsed
                .as_ref()
                .and_then(|parsed| singular(parsed, path, "attach select"))
                .cloned(),
            None => parsed,
        };
        if let Some(parsed) = parsed {
            return read(folded(parsed));
        }
    }
    // The span name, where the conventions put the same fact.
    if let Some(prefix) = attach.or_span_name_after()
        && let Some(rest) = ctx.span_name.strip_prefix(prefix)
    {
        let trimmed = rest.trim();
        if !trimmed.is_empty() {
            return read(json!(trimmed));
        }
    }
    Attachment::Nothing
}

/// Whether an indexed entry's bucket holds the members a rule requires.
///
/// The bucket rather than the span's whole attribute map: asked per index, a full scan is quadratic in the
/// family's size, and every answer is a property of this index's keys alone.
pub(super) fn bucket_members_present<'a>(
    members: &[(&'a str, &String)],
    entry_member: Option<&str>,
    require: &MemberRequirements,
) -> bool {
    // Names are relative to the *subject*, which is the entry or a sub-level of it.
    let within = entry_member.map(|nested| format!("{nested}."));
    let relative = |member: &'a str| -> Option<&'a str> {
        match &within {
            Some(prefix) => member.strip_prefix(prefix.as_str()),
            None => Some(member),
        }
    };
    let present = |requirement: &super::schema::MemberRequirement| {
        let nested = format!("{}.", requirement.name);
        let exact = members
            .iter()
            .any(|(member, _)| relative(member) == Some(requirement.name.as_str()));
        let under = || {
            members
                .iter()
                .any(|(member, _)| relative(member).is_some_and(|m| m.starts_with(nested.as_str())))
        };
        match requirement.presence {
            MemberPresence::Exact => exact,
            MemberPresence::Nested => under(),
            MemberPresence::Either => exact || under(),
        }
    };
    (require.all_of.is_empty() || require.all_of.iter().all(present))
        && (require.any_of.is_empty() || require.any_of.iter().any(present))
}

/// Assemble a composed message, or `None` where the span supplied no member.
///
/// Emitted only when at least one *source* member was filled - the trailing literals are not evidence of
/// anything, so a rule whose sources all missed would otherwise emit a message consisting of a role.
pub(super) fn composed(
    compose: &CompiledCompose,
    ctx: &MessageContext<'_>,
    read: &mut Vec<OwnedCarrier>,
) -> Option<JsonValue> {
    let attrs = ctx.span_attrs;
    let mut object = serde_json::Map::new();

    for compiled in &compose.members {
        let member = &compiled.spec;
        if let Some(prefix) = &member.sweep_prefix {
            // Sorted before insertion. The attribute map's iteration order is randomised per process, and
            // this object is serialised with insertion order preserved and *persisted* - so an unsorted
            // sweep writes different bytes for the same span on different runs. The equivalence oracle
            // cannot see it: both implementations walk the same map in the same process.
            let mut swept: Vec<(&str, &String)> = attrs
                .iter()
                .filter_map(|(key, value)| {
                    key.strip_prefix(prefix.as_str())
                        .filter(|suffix| !member.except.iter().any(|skip| skip == suffix))
                        .map(|suffix| (suffix, value))
                })
                .collect();
            swept.sort_unstable_by_key(|(suffix, _)| *suffix);
            for (suffix, value) in swept {
                read.push(OwnedCarrier::attribute(&format!("{prefix}{suffix}")));
                object.insert(suffix.to_string(), sniffed_value(value));
            }
            continue;
        }
        let Some(name) = &member.as_member else {
            continue;
        };
        let direct = member
            .from_any_of
            .iter()
            .find_map(|key| attrs.get(key).map(|raw| (key, raw)))
            .and_then(|(key, raw)| {
                // The carrier this member actually read - recorded **after** the parse, so a member whose
                // payload does not parse neither contributes nor takes the carrier off the table. Recorded
                // before, a malformed compose member was owned while the *same* malformed carrier read by an
                // ordinary rule was not: one asymmetry, and the compose silently suppressed another dialect's
                // reading of junk it could not read either. A rule that means to take a payload away without
                // emitting has `claim` for it.
                let value = parse_value(raw, member.parse.unwrap_or(ParseMode::Text))?;
                read.push(OwnedCarrier::attribute(key));
                Some(value)
            });
        let value = direct.or_else(|| {
            // The conditional last resort: a key that is not this dialect's own, read only on evidence
            // that the span is one of its spans.
            let fallback = member.fallback.as_ref()?;
            let gate = compiled.fallback_gate.as_ref()?;
            if !super::span_conditions::holds(
                gate,
                &super::span_conditions::SpanSubject {
                    span_name: ctx.span_name,
                    attrs: ctx.gate_attrs,
                    scope_name: None,
                    resource: None,
                    scope_version: None,
                },
            ) {
                return None;
            }
            let raw = attrs.get(&fallback.from)?;
            let value = parse_value(raw, fallback.parse.unwrap_or(ParseMode::Text))?;
            read.push(OwnedCarrier::attribute(&fallback.from));
            Some(value)
        });
        if let Some(value) = value {
            object.insert(name.clone(), value);
        }
    }

    if object.is_empty() {
        return None;
    }
    for (member, literal) in &compose.trailing {
        object.insert(member.clone(), literal.clone());
    }
    Some(JsonValue::Object(object))
}

/// A leading `[TAG]\n` marker split from its body. The tag is trimmed; the body is the producer's bytes after
/// the tag line, untouched.
///
/// The tag is recognised only with the newline: a body that merely opens with a bracket is not a tagged
/// section, and treating it as one would swallow its first line.
pub(super) fn split_bracket_tag(value: &str) -> (Option<&str>, &str) {
    match value
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]\n"))
    {
        Some((tag, body)) => (Some(tag.trim()), body),
        None => (None, value),
    }
}

/// The messages a tagged text carrier yields, one per section.
/// Each section's message, with **which route** built it.
///
/// `SectionRoute.id` is a required declaration and was discarded here: two routes of one rule produced
/// emissions carrying identical evidence, so a diagnostic could name the rule and not the route - which is
/// exactly what a reader needs when `claude-agent-sdk.new_context`'s `tool_result` and `as_user` routes
/// disagree.
pub(super) fn sectioned(
    raw: &str,
    spec: &SectionsSpec,
    attrs: &HashMap<String, String>,
) -> Vec<(String, JsonValue)> {
    let mut out = Vec::new();
    let attribute = |source: &schema::SourceName| -> Option<&String> {
        source
            .0
            .strip_prefix("attr:")
            .and_then(|key| attrs.get(key))
    };
    let mut sections: Vec<&str> = match spec.max_sections {
        Some(most) => raw.splitn(most, spec.split_on.as_str()).collect(),
        None => raw.split(spec.split_on.as_str()).collect(),
    };
    // A carrier shorter than its stated length was cut, and only its last section can be the cut one.
    if let Some(witness) = &spec.truncated_unless_length
        && let Some(stated) =
            attribute(&witness.source).and_then(|v| v.trim().parse::<usize>().ok())
        && witness.counts.length_of(raw) < stated
    {
        sections.pop();
    }
    let carried: Vec<&str> = spec
        .skip_sections_equal_to
        .iter()
        .filter_map(attribute)
        .map(|value| value.trim())
        .collect();
    for section in sections {
        if carried.contains(&section.trim()) {
            continue;
        }
        let (tag, body) = split_bracket_tag(section);
        // A section of nothing but whitespace holds no turn, whatever a route would do with it.
        if body.trim().is_empty() {
            continue;
        }
        // The first route whose prefix the tag carries, else the default.
        let matched = spec
            .routes
            .iter()
            .find_map(|route| match &route.tag_prefix {
                Some(prefix) => tag
                    .and_then(|t| t.strip_prefix(prefix.as_str()))
                    .map(|rest| (route, Some(rest.trim()))),
                None => Some((route, None)),
            });
        let Some((route, capture)) = matched else {
            continue;
        };
        if !route.skip_when.is_empty() {
            // The section as a value, so one predicate vocabulary answers this too.
            let subject = json!({
                "capture": capture.map(JsonValue::from).unwrap_or(JsonValue::Null),
                "body": body,
            });
            if predicates_hold(&subject, &route.skip_when) {
                continue;
            }
        }
        let mut message = serde_json::Map::new();
        message.insert("role".to_string(), json!(route.role));
        match &route.block {
            Some(block) => {
                let mut object = serde_json::Map::new();
                object.insert("type".to_string(), json!(block.block_type));
                if let Some(member) = &block.capture_as
                    && let Some(captured) = capture
                {
                    object.insert(member.clone(), json!(captured));
                }
                object.insert("content".to_string(), json!(body));
                message.insert(
                    "content".to_string(),
                    JsonValue::Array(vec![JsonValue::Object(object)]),
                );
            }
            None => {
                message.insert("content".to_string(), json!(body));
            }
        }
        out.push((route.id.clone(), JsonValue::Object(message)));
    }
    out
}

#[cfg(test)]
mod section_option_tests {
    use super::*;

    fn spec(extra: serde_json::Value) -> SectionsSpec {
        let mut value = serde_json::json!({
            "split_on": "\n\n",
            "routes": [{"id": "all", "role": "system"}],
        });
        value
            .as_object_mut()
            .expect("an object")
            .extend(extra.as_object().expect("an object").clone());
        serde_json::from_value(value).expect("the sections spec parses")
    }

    fn bodies(raw: &str, spec: &SectionsSpec, attrs: &[(&str, &str)]) -> Vec<String> {
        let attrs: HashMap<String, String> = attrs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        sectioned(raw, spec, &attrs)
            .into_iter()
            .map(|(_, message)| message["content"].as_str().expect("text").to_string())
            .collect()
    }

    #[test]
    fn max_sections_keeps_every_later_separator_in_the_last_section() {
        let raw = "header\n\npreamble\n\nthe prompt\n\nwith a blank line";
        assert_eq!(bodies(raw, &spec(serde_json::json!({})), &[]).len(), 4);
        assert_eq!(
            bodies(raw, &spec(serde_json::json!({"max_sections": 3})), &[]),
            ["header", "preamble", "the prompt\n\nwith a blank line"]
        );
    }

    #[test]
    fn a_carrier_shorter_than_its_stated_length_loses_its_cut_section() {
        let raw = "header\n\npreamble\n\nthe pro";
        let cut = |counts: &str| {
            spec(
                serde_json::json!({"truncated_unless_length": {"source": "attr:len", "counts": counts}}),
            )
        };
        // Complete: as long as stated.
        let whole = raw.chars().count().to_string();
        assert_eq!(bodies(raw, &cut("chars"), &[("len", &whole)]).len(), 3);
        // Cut: the stated length is longer, so the last section is partial and is left out.
        assert_eq!(
            bodies(raw, &cut("chars"), &[("len", "500")]),
            ["header", "preamble"]
        );
        // Absent or unreadable: the carrier stands as it is.
        assert_eq!(bodies(raw, &cut("chars"), &[]).len(), 3);
        assert_eq!(bodies(raw, &cut("chars"), &[("len", "many")]).len(), 3);
    }

    /// The unit decides the answer on multibyte text: `é` is one character, one UTF-16 unit and two bytes, and
    /// `😀` one character, two UTF-16 units and four bytes.
    #[test]
    fn the_stated_length_is_compared_in_its_declared_unit() {
        let raw = "é\n\n😀";
        assert_eq!(schema::LengthUnit::Chars.length_of(raw), 4);
        assert_eq!(schema::LengthUnit::Utf16Units.length_of(raw), 5);
        assert_eq!(schema::LengthUnit::Bytes.length_of(raw), 8);
        let with = |counts: &str, stated: &str| {
            let spec = spec(serde_json::json!({
                "truncated_unless_length": {"source": "attr:len", "counts": counts}
            }));
            bodies(raw, &spec, &[("len", stated)]).len()
        };
        // Five is complete for a JavaScript producer and cut for a Python one.
        assert_eq!(with("utf16_units", "5"), 2);
        assert_eq!(with("chars", "5"), 1);
        assert_eq!(with("bytes", "8"), 2);
        assert_eq!(with("bytes", "9"), 1);
    }

    #[test]
    fn a_section_another_attribute_states_whole_is_left_out() {
        let raw = "header\n\nthe prompt\n\nreminder";
        let spec = spec(serde_json::json!({"skip_sections_equal_to": ["attr:prompt"]}));
        assert_eq!(
            bodies(raw, &spec, &[("prompt", " the prompt\n")]),
            ["header", "reminder"]
        );
        // Only the whole value: a section merely containing it stays.
        assert_eq!(bodies(raw, &spec, &[("prompt", "the")]).len(), 3);
        assert_eq!(bodies(raw, &spec, &[]).len(), 3);
    }

    /// A body is the producer's bytes: only the tag line and the separator are removed, so the whitespace and
    /// multibyte text around a turn survive.
    #[test]
    fn a_section_body_keeps_its_own_bytes() {
        let spec = spec(serde_json::json!({
            "routes": [{"id": "u", "tag_prefix": "user", "role": "user"}],
        }));
        assert_eq!(
            bodies(
                "[user]\n  Grüße, 😀 each day.\n\n[user]\n\t日本語 \n",
                &spec,
                &[]
            ),
            ["  Grüße, 😀 each day.", "\t日本語 \n"]
        );
        // The tag is the line it is on, so a body opening with a bracket keeps it.
        assert_eq!(
            bodies("[user]\n[draft]\nné\n", &spec, &[]),
            ["[draft]\nné\n"]
        );
        // A section of whitespace alone holds no turn.
        assert!(bodies("[user]\n \n", &spec, &[]).is_empty());
    }
}
