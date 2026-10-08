use super::*;

/// The observations one rule finds on a span.
///
/// Separate from the plan's loop so a branch set's sub-readings run through exactly the same path as a
/// top-level rule - a second evaluator would be a second place for the two to drift.
pub(super) fn emit_rule<'p>(
    rule: &'p CompiledMessageRule,
    ctx: &MessageContext<'_>,
) -> Vec<Emission<'p>> {
    // A branch set is several readings with a local order between them: the primaries, then the fallbacks
    // only if those found nothing, then the unconditional ones. Evaluated here, not in `run`, so the
    // metadata path sees it too and the primary-empty decision is made on *all* emissions - a primary
    // whose only output is a tool list still counts as matched, which is what stops a tools-only request
    // from wrongly taking the fallback.
    if let Some(set) = &rule.branch_set {
        let mut out = Vec::new();
        if !gates_allow(rule, ctx) {
            return out;
        }
        // Per sub-rule and **per axis**, because the parent declares no carrier and its own flag says
        // nothing about what a sub-rule reads. A leaf that may not read a tool span as a conversation may
        // still state that span's tools, so its metadata emissions are kept and its message emissions are
        // not - a whole-leaf boolean let a mixed-axis leaf through on the strength of its metadata and `run`
        // then retained its messages.
        let from = |sub: &'p CompiledMessageRule| -> Vec<Emission<'p>> {
            let forbidden = ctx.is_tool_span && !sub.reads_tool_spans;
            emit_rule(sub, ctx)
                .into_iter()
                .filter(|emission| {
                    !forbidden
                        || matches!(
                            emission.target,
                            EmitTarget::ToolDefinitions | EmitTarget::ToolNames
                        )
                })
                .collect()
        };
        for sub in &set.primary {
            out.extend(from(sub));
        }
        // "Every primary reading came up empty" is asked **per axis**, not over every emission.
        //
        // A branch set may hold leaves that answer different questions about the same carrier: one dialect's
        // primary reads its serialised request as the conversation *and* reads the tools it was offered out of
        // the same attribute. Judged over all emissions, a request carrying tools and no messages made the
        // branch non-empty, so the fallback did not run - and the message path then filtered the tool
        // definition out, so the span reported **no message at all** and the tool call's arguments were lost.
        //
        // So a fallback leaf runs when nothing of *its* kind was produced. That is what the declaration says:
        // read this instead, if the primaries found none of what you are asking for.
        let produced: std::collections::BTreeSet<EmitTarget> =
            out.iter().map(|emission| emission.target).collect();
        for sub in &set.fallback {
            if possible_targets(sub)
                .into_iter()
                .any(|target| produced.contains(&target))
            {
                continue;
            }
            out.extend(from(sub));
        }
        for sub in &set.always {
            out.extend(from(sub));
        }
        return out;
    }
    let mut out = Vec::new();
    if !gates_allow(rule, ctx) {
        return out;
    }
    if let Some(compose) = &rule.compose {
        // Every physical attribute the compose read, so the emission owns them all. Owning only the
        // synthetic tag left each consumed attribute free for another dialect to read as conversation.
        let mut read_carriers = Vec::new();
        if let Some(value) = composed(compose, ctx, &mut read_carriers).filter(|value| {
            // Judged once the members are together: a name a dialect reported may not be a tool anyone can
            // call, and only the assembled object shows it.
            predicates_hold(value, &compose.require)
        }) {
            // The canonical tool-definition shape, where the assembled members are one tool rather than a
            // message. Wrapped here because the shape is ours and the members are the dialect's.
            let value = if compose.as_tool_definition {
                json!([{"type": "function", "function": value}])
            } else {
                value
            };
            // The synthetic tag is **not** owned: `owns` is what the span carried and this is the name the
            // engine gives the assembled result. Claiming it let one compose suppress another that read
            // entirely different carriers.
            out.push(Emission {
                rendering: false,
                rule_id: &rule.rule_id,
                evidence: rule_evidence(rule, &[]),
                carrier: EmittedCarrier::Attribute(compose.tag.as_str()),
                owns: read_carriers,
                target: rule.target,
                value,
            });
        }
        return out;
    }
    // Tool definitions written as a language's `repr`: the grammar is sealed, its vocabulary declared.
    if let Some(spec) = &rule.tool_repr {
        for (attribute, raw) in carrier_texts(rule, ctx) {
            if let Some(parsed) = parse_value(raw, rule.parse.unwrap_or(ParseMode::Json))
                && let Some(tools) = super::tool_repr::tools_from_carrier(&parsed, spec)
            {
                out.push(Emission {
                    rendering: false,
                    rule_id: &rule.rule_id,
                    evidence: rule_evidence(rule, &[]),
                    carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(attribute)),
                    owns: OwnedCarrier::just(attribute),
                    target: rule.target,
                    value: JsonValue::Array(tools),
                });
            }
        }
        return out;
    }
    if let Some(family) = rule.read.indexed_family.as_deref() {
        let entries = indexed_entries(
            ctx.span_attrs,
            family,
            &rule.read,
            rule.require_members.as_ref(),
        )
        .into_iter()
        .filter(|entry| predicates_hold(&entry.value, &rule.read.entry_require))
        .collect::<Vec<_>>();
        // A result set is one observation. Its entries are the array, and the envelope says what the array
        // is - so the whole family is tagged once rather than one carrier per document.
        if rule.aggregate_into_array {
            if entries.is_empty() {
                return out;
            }
            // Every key the aggregate consumed, plus each entry's tag and the family's own name. Owning the
            // family name alone left every physical member free for another rule, and the entry tags are
            // names this engine assembled rather than keys a producer wrote.
            let mut owns: Vec<OwnedCarrier> = Vec::new();
            for entry in &entries {
                owns.push(OwnedCarrier::attribute(&entry.carrier));
                owns.extend(
                    entry
                        .consumed
                        .iter()
                        .map(|key| OwnedCarrier::attribute(key)),
                );
            }
            owns.push(OwnedCarrier::attribute(family));
            owns.sort_unstable();
            owns.dedup();
            let array = JsonValue::Array(entries.into_iter().map(|entry| entry.value).collect());
            let value = match &rule.wrap {
                Some(wrap) => match wrapped(array, wrap, ctx, None) {
                    Some(value) => value,
                    None => return out,
                },
                None => array,
            };
            out.push(Emission {
                rendering: false,
                rule_id: &rule.rule_id,
                evidence: rule_evidence(rule, &[]),
                carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(family)),
                owns,
                target: rule.target,
                value,
            });
            return out;
        }
        for entry in entries {
            // The entry's tag *and* every key it read. The tag alone is a name for the entry, not a key, so
            // it conflicted with nothing a second rule could reach.
            let mut owns = OwnedCarrier::just(&entry.carrier);
            owns.extend(
                entry
                    .consumed
                    .iter()
                    .map(|key| OwnedCarrier::attribute(key)),
            );
            owns.sort_unstable();
            owns.dedup();
            out.push(Emission {
                rule_id: &rule.rule_id,
                evidence: rule_evidence(rule, &[]),
                owns,
                carrier: EmittedCarrier::Owned(entry.carrier),
                target: rule.target,
                rendering: rule
                    .read
                    .rendering
                    .as_ref()
                    .is_some_and(|condition| predicates_hold(&entry.value, condition)),
                value: entry.value,
            });
        }
        return out;
    }
    let (attribute, parsed, owns) = match rule.read.family.as_deref() {
        Some(prefix) => match family_object(ctx.span_attrs, prefix) {
            Some((object, keys)) => (
                prefix,
                object,
                keys.into_iter().map(OwnedCarrier::attribute).collect(),
            ),
            None => return out,
        },
        None => {
            // Event carriers are read from a span's events, not its attributes; no declared rule needs
            // one yet, and probing the attribute map for an event name would silently match nothing.
            let Some((attribute, raw)) = resolve_attribute(&rule.read, ctx.span_attrs) else {
                return out;
            };
            if !predicates_hold(&JsonValue::String(raw.to_string()), &rule.raw_where) {
                return out;
            }
            // An array-valued carrier read element by element, in declared passes.
            if let Some(elements) = &rule.elements {
                let Some(parsed) = parse_value(raw, rule.parse.unwrap_or(ParseMode::Json)) else {
                    return out;
                };
                for (carrier, value, clause) in element_passes(&parsed, elements) {
                    // An event carrier is kept as one: carrier semantics are looked up by kind, so reporting an
                    // event as an attribute changes what the pipeline reads it as evidence of.
                    let tagged = if elements.tags_are_events {
                        EmittedCarrier::OwnedEvent(carrier)
                    } else {
                        EmittedCarrier::Owned(carrier)
                    };
                    out.push(Emission {
                        rendering: false,
                        rule_id: &rule.rule_id,
                        evidence: rule_evidence(rule, &clause),
                        // The array attribute is what was read; each element's tag is a name for one of its parts.
                        owns: OwnedCarrier::just(attribute),
                        carrier: tagged,
                        target: rule.target,
                        value,
                    });
                }
                return out;
            }
            // A text carrier read as tagged sections, each emitted on its own.
            if let Some(sections) = &rule.sections {
                for (route, value) in sectioned(raw, sections, ctx.span_attrs) {
                    out.push(Emission {
                        rendering: false,
                        rule_id: &rule.rule_id,
                        evidence: rule_evidence(rule, &[vec![route]]),
                        carrier: EmittedCarrier::Attribute(
                            rule.tag_as.as_deref().unwrap_or(attribute),
                        ),
                        owns: OwnedCarrier::just(attribute),
                        target: rule.target,
                        value,
                    });
                }
                return out;
            }
            let Some(parsed) = parse_value(raw, rule.parse.unwrap_or(ParseMode::Json)) else {
                return out;
            };
            (attribute, parsed, OwnedCarrier::just(attribute))
        }
    };
    // A state object its nodes write into: the readings are applied at every node of a bounded walk, so a
    // conversation nested a level or two down is found without trawling the payload for anything
    // message-shaped.
    // The aggregate path passes **no** construction: its entries become one array and the rule's envelope
    // wraps that array, once. Compilation refuses a per-reading envelope beside an aggregate, so nothing can be
    // lost by not building here.
    let build = (!rule.aggregate_into_array).then_some(Construction {
        rule_wrap: rule.wrap.as_ref(),
        ctx,
        root: &parsed,
    });
    let readings = match &rule.walk {
        Some(walk) => walked_readings(&parsed, rule, walk, build),
        None => all_readings(&parsed, rule, build).built,
    };
    // A tool list is a set, not a sequence of messages: the whole list is one observation, and emitting one
    // per tool would make each look like a separate declaration.
    if rule.aggregate_into_array {
        if readings.is_empty() {
            return out;
        }
        // The aggregate is built from **every** reading, so its evidence is every clause that contributed -
        // which is why an emission carries a set rather than one path.
        let mut contributing: Vec<Vec<String>> = Vec::new();
        let mut values = Vec::new();
        for (value, _, path, _) in readings {
            if !contributing.contains(&path) {
                contributing.push(path);
            }
            values.push(value);
        }
        let assembled = JsonValue::Array(values);
        // **The rule's envelope wraps the assembled array, once.** It used to be discarded here while the
        // indexed-family aggregate applied it - so the same two declarations meant different things depending on
        // the read form. Per-reading envelopes are refused beside an aggregate, so this is the only one there
        // can be, and an envelope that cannot be built is no observation rather than a bare array under a
        // message tag.
        let value = match &rule.wrap {
            Some(wrap) => match wrapped(assembled, wrap, ctx, Some(&parsed)) {
                Some(built) => built,
                None => return out,
            },
            None => assembled,
        };
        out.push(Emission {
            rendering: false,
            rule_id: &rule.rule_id,
            evidence: rule_evidence(rule, &contributing),
            carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(attribute)),
            owns: owns.clone(),
            target: rule.target,
            value,
        });
        return out;
    }
    // Already built: an envelope that could not be made was a reading that produced nothing, decided inside
    // the coalesce so the alternatives after it and the rule's `fallback` still got their turn.
    //
    // **Capped**, for the reason the walk is: a rule reading an array emits one observation per element, so a
    // payload holding a hundred thousand elements is a hundred thousand messages from one span - which no
    // producer means and no reader can use. Server policy rather than a declaration, and reported so a truncated
    // answer is not mistaken for a complete one.
    if readings.len() > sideseat_core::constants::RULE_MAX_EMISSIONS_PER_CARRIER {
        tracing::warn!(
            target: "sideseat::rules",
            rule = %rule.rule_id,
            carrier = %attribute,
            found = readings.len(),
            limit = sideseat_core::constants::RULE_MAX_EMISSIONS_PER_CARRIER,
            "a carrier yielded more observations than this server reports from one; the rest are dropped"
        );
    }
    for (value, per_reading_target, clause, rendering) in readings
        .into_iter()
        .take(sideseat_core::constants::RULE_MAX_EMISSIONS_PER_CARRIER)
    {
        out.push(Emission {
            rule_id: &rule.rule_id,
            evidence: rule_evidence(rule, std::slice::from_ref(&clause)),
            carrier: EmittedCarrier::Attribute(rule.tag_as.as_deref().unwrap_or(attribute)),
            owns: owns.clone(),
            target: per_reading_target.unwrap_or(rule.target),
            value,
            rendering,
        });
    }

    out
}

/// A dotted family of span attributes as one object, keyed by what follows the prefix, with the keys it
/// read; `None` when the span has none of them.
///
/// Members are not sniffed by shape: a string attribute that happens to spell a number reads as that
/// number. The flattening instrumentations write each value's type beside it, and a reading that needs
/// the distinction can select it.
fn family_object<'s>(
    attrs: &'s HashMap<String, String>,
    prefix: &str,
) -> Option<(JsonValue, Vec<&'s str>)> {
    let mut keys: Vec<&str> = attrs
        .keys()
        .map(String::as_str)
        .filter(|key| key.len() > prefix.len() && key.starts_with(prefix))
        .collect();
    if keys.is_empty() {
        return None;
    }
    keys.sort_unstable();
    let object = keys
        .iter()
        .map(|key| {
            // Attribute values arrive as text, an integer included, so a member that spells JSON - a
            // number, a boolean, an object - is that value; anything else is the text it is.
            let raw = &attrs[*key];
            let value = serde_json::from_str::<JsonValue>(raw)
                .unwrap_or_else(|_| JsonValue::String(raw.clone()));
            (key[prefix.len()..].to_string(), value)
        })
        .collect();
    Some((JsonValue::Object(object), keys))
}
