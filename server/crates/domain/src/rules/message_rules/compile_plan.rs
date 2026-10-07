use super::*;

/// Compile every asset's message rules into one plan.
pub fn compile(
    assets: &super::super::assets::ParsedAssets,
) -> Result<MessagePlan, MessageCompileError> {
    let mut rules: Vec<CompiledMessageRule> = Vec::new();
    let mut seen_ids: HashMap<String, ()> = HashMap::new();

    // Fragments first, across every asset: a rule may reference one defined in another file, which is what
    // makes a shared dialect table shared. Recognised events are collected in the same pass, for the same
    // reason - a rule may name an event another file recognises.
    let mut fragments: HashMap<String, Vec<Alternative>> = HashMap::new();
    // The spellings a rule may state as a role, from the same corpus: the ruleset this builds is not there to
    // ask.
    let roles = super::super::declared_role_meanings(assets.files());
    let mut recognised_events: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut raw_forms: std::collections::BTreeMap<String, super::schema::RawEventForm> =
        std::collections::BTreeMap::new();
    for file in assets.files() {
        if file.message_events.iter().any(|e| e.name.is_empty()) {
            return Err(MessageCompileError::Inexpressible {
                rule: format!("{}.message_events", file.id),
                detail: "declares an event with no name, which would make every unnamed event a message \
                         event",
            });
        }
        recognised_events.extend(file.message_events.iter().map(|e| e.name.clone()));
        // The raw form travels **with the plan**, from the same declarations the recognition set comes from.
        // `from_event` used to read `ruleset().message_events`, a global, which made the one policy this entry
        // point acts on unreachable from a probe plan - so a claim-only reading of a container event could not
        // be tested at all. A disagreement between the two is refused by `compile_message_events`; here the
        // last write wins for a *repeat*, which that refusal has already excluded.
        for event in &file.message_events {
            raw_forms.insert(event.name.clone(), event.raw.unwrap_or_default());
        }
        for (name, fragment) in &file.fragments {
            if fragment.cases.is_empty() {
                return Err(MessageCompileError::Inexpressible {
                    rule: format!("{}.{name}", file.id),
                    detail: "a fragment declares no cases",
                });
            }
            // One level, no recursion: a fragment's cases may not reference a fragment.
            if fragment.cases.iter().any(|c| c.then_fragment.is_some()) {
                return Err(MessageCompileError::Inexpressible {
                    rule: format!("{}.{name}", file.id),
                    detail: "a fragment's own cases may not reference a fragment. One level, deliberately: \
                             composition would be a compile-time expansion - splice the named fragment's \
                             cases in place, refuse a cycle, bound the depth - and no asset needs it, so \
                             building it now would add a capability nothing exercises, which is the defect \
                             this engine refuses everywhere else. Lift it when a second level is what an \
                             asset actually wants to say",
                });
            }
            if fragments
                .insert(format!("{}.{name}", file.id), fragment.cases.clone())
                .is_some()
            {
                return Err(MessageCompileError::Inexpressible {
                    rule: format!("{}.{name}", file.id),
                    detail: "a fragment of this name is already declared",
                });
            }
        }
    }

    for file in assets.files() {
        for rule in &file.messages {
            // Branch leaves too, not only top-level rules. A leaf's id is what `keep_unclaimed` uses to
            // tell "this rule reading its own carrier again" from "a second rule reading it", so two leaves
            // sharing an id would make that check silently pass a double read.
            let ids = std::iter::once(&rule.id).chain(
                rule.branch_set
                    .iter()
                    .flat_map(|set| {
                        set.primary
                            .iter()
                            .chain(&set.fallback_if_primary_empty)
                            .chain(&set.always)
                    })
                    .map(|sub| &sub.id),
            );
            for id in ids {
                if seen_ids.insert(id.clone(), ()).is_some() {
                    return Err(MessageCompileError::DuplicateRuleId { rule: id.clone() });
                }
            }
            // A top-level rule's position among the others is policy somebody owns, so it is stated.
            if rule.priority.is_none() {
                return Err(MessageCompileError::Inexpressible {
                    rule: rule.id.clone(),
                    detail: "a top-level rule must declare `priority`, which is its position among \
                             the others",
                });
            }
            rules.push(compile_rule(&file.id, rule, &fragments, &roles)?);
        }
    }

    // Predicates, checked once over every place a compiled rule holds one - after fragments and extra
    // cases are inlined, which is the only point at which they are all visible.
    for rule in &rules {
        if let Some(detail) = predicate_sets(rule).into_iter().find_map(predicate_defect) {
            return Err(MessageCompileError::Inexpressible {
                rule: rule.rule_id.clone(),
                detail,
            });
        }
        if let Some(detail) = wraps(rule)
            .into_iter()
            .flat_map(wrap_attachments)
            .find_map(attach_defect)
        {
            return Err(MessageCompileError::Inexpressible {
                rule: rule.rule_id.clone(),
                detail,
            });
        }
    }

    // An event name no asset recognises makes a rule *dead*: recognition rejects the event before the plan
    // is asked, so the rule compiles and never runs. That is the failure declaring recognition was meant to
    // remove, so it is refused rather than left to be discovered.
    for rule in &rules {
        if rule
            .source
            .event_names()
            .iter()
            .any(|name| !recognised_events.contains(name))
        {
            return Err(MessageCompileError::Inexpressible {
                rule: rule.rule_id.clone(),
                detail: "names an event no asset recognises, so it would never run - declare it in a \
                         `message_events` list",
            });
        }
    }

    rules.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.rule_id.cmp(&b.rule_id))
    });

    // A shared priority is refused **within an ordering arena**, because the tie-break above is the rule *id*:
    // renaming a rule would change which of two contenders reads a carrier, and a rule id must not be a
    // control-flow primitive.
    //
    // Not refused globally, because five pairs in the shipped assets share one legitimately: each puts a
    // *message* rule beside a *metadata* rule, and their orders are independent - a tool definition is not a
    // reading of the conversation and the two paths never contend. So an arena is a set of rules whose relative
    // order is observable: the same stage, an overlapping output axis, and either both reading a span's
    // attributes or both reading events whose names intersect. Pairwise, because that relation is not an
    // equivalence: grouping it into components would refuse ties between rules that never contend.
    if let Some((rule, other)) =
        super::super::precedence::shared_priority(&rules, |rule| rule.priority, share_an_arena)
    {
        return Err(MessageCompileError::Inexpressible {
            rule: format!("{} and {}", rule.rule_id, other.rule_id),
            detail: "share a priority in one ordering arena, so which of them reads a contested carrier is \
                     decided by comparing their *ids* - renaming a rule would change the answer",
        });
    }

    // Two rules must not claim one carrier, in either direction.
    //
    // Previously this compared "the carrier a rule reads" and missed four real conflicts: a `tag_as` that
    // emits a name the rule never read, an indexed family against an exact key it generates, a `compose`
    // that consumes a carrier another rule emits, and a sweep overlapping an exact source. Comparing the
    // *consumed* and *emitted* sets asks the question that matters - who owns this carrier - rather than
    // one convenient projection of it.
    // Claiming is a message-axis mechanism: `run` claims carriers, `tool_definitions` does not. So two
    // rules contend only when both can emit on the message axis - a message rule and a pure
    // tool-definition rule reading the same carrier is the co-located case (one conversation, one tool
    // list) that routing-by-emission handles, not a conflict.
    // `possible_targets` already looks everywhere a target can be declared - a branch set's leaves, a
    // fragment's cases, a selection point's extra cases - so this is one question with one answer, and the
    // metadata side asks it of the same function.
    // **Per output axis.** Two rules reading one carrier contend on the axis they both emit on, and only that
    // one: a dialect stating its tools on the carrier another rule reads as a conversation is two true
    // statements, which is why the check was written for the message axis. But it was written for the message
    // axis *alone*, so two rules reading one carrier and both emitting tool definitions compiled - and the
    // metadata path does no claiming, so both survived and their rank silently became precedence somewhere
    // downstream.
    //
    // `tool_repr` is excluded from the message axis because a `repr` grammar reads tool schemas, never a
    // conversation - it is a metadata reader whatever else the rule declares.
    /// Whether a rule can emit on one output axis.
    type EmitsOnAxis = fn(&CompiledMessageRule) -> bool;
    let axes: [EmitsOnAxis; 3] = [
        |rule: &CompiledMessageRule| {
            rule.tool_repr.is_none()
                && possible_targets(rule)
                    .into_iter()
                    .any(|target| matches!(target, EmitTarget::Message | EmitTarget::Claim))
        },
        |rule: &CompiledMessageRule| {
            rule.tool_repr.is_some()
                || possible_targets(rule).contains(&EmitTarget::ToolDefinitions)
        },
        |rule: &CompiledMessageRule| possible_targets(rule).contains(&EmitTarget::ToolNames),
    ];
    for (i, a) in rules.iter().enumerate() {
        for b in &rules[i + 1..] {
            if !axes.iter().any(|emits| emits(a) && emits(b)) {
                continue;
            }
            // An event rule reads an *event's* attributes; a span rule reads the span's. Two different maps,
            // so a key appearing in both is two different carriers - `gen_ai.input.messages` is a span
            // attribute for one convention and an attribute *of* the inference-details event for another.
            if a.source.event_names().is_empty() != b.source.event_names().is_empty() {
                continue;
            }
            // Two event rules contend only if they can apply to the same event.
            if !a.source.event_names().is_empty()
                && !a
                    .source
                    .event_names()
                    .iter()
                    .any(|name| b.source.event_names().contains(name))
            {
                continue;
            }
            // **Starvation**, which the conflict check above deliberately excuses and must not.
            //
            // Every excuse it makes rests on "the two take turns and the ranks decide": a conditional claim
            // yields on spans its condition excludes, so the lower rank simply goes first. That is sound when
            // the loser's emission owns the one carrier it lost. It is false for an **all-or-nothing** reading,
            // where an emission is accepted only if its whole ownership set is free - the loser is dropped
            // *whole*, and the carriers the taker never wanted end up owned by nobody.
            //
            // Directional: `a` holds the earlier rank, so it is `a` that can starve `b`. Asked of every
            // multi-owner reading, not only `compose`: an indexed family's entry owns its own members, an
            // aggregate owns every entry, and an overlay owns both sides of the join. In the indexed-family
            // case, a conditional rank-1 rule reading `family.0.role` leaves the entry unable to
            // take `family.0.content`, which then reaches nobody.
            //
            // Asked **before** the stage exemption below, deliberately. That exemption is sound for the
            // question it guards - two stages sharing a carrier are safe because the fallback *inherits* what
            // the dialect stage claimed - and inheritance is exactly what makes cross-stage starvation real: a
            // multi-owner reading at the fallback stage arrives with the dialect's claims already in its claim
            // set, so a dialect rule that took one of its keys drops the whole reading.
            for owned in owned_all_or_nothing(b) {
                if let Some(taken) = consumed_carriers(a)
                    .iter()
                    .chain(emitted_carriers(a).iter())
                    .find(|consumed| consumed.pattern.overlaps(&owned))
                {
                    return Err(MessageCompileError::StarvedReading {
                        starved: b.rule_id.clone(),
                        taker: a.rule_id.clone(),
                        carrier: taken.pattern.describe(),
                    });
                }
            }
            // Two stages may share a carrier, and the reason is *not* that they never run together - a
            // generation span whose answer is unaccounted for reads the fallback after a dialect produced
            // something, which is exactly when they do. What makes the pair safe is that the fallback
            // **inherits** what the dialect stage read, including a carrier it only *claimed*: `fallback`
            // takes those carriers and starts its claim set from them. So the guarantee is enforced at
            // evaluation, not assumed here.
            if a.source.stage() != b.source.stage() {
                continue;
            }
            // A conditional claim is not a dead rule: it yields on spans its condition excludes, and the
            // ranks decide which is tried first. Only two *unconditional* claims on one carrier are a defect.
            //
            // Asked per carrier, not per rule. A rule that reads an indexed family unconditionally and joins
            // against a side payload only where a witness holds is conditional about the payload and not
            // about the family - and as a rule-wide flag it waived every conflict the rule was in, so a
            // second rule reading one of the family's own keys was accepted while being permanently dead.
            // What runtime ownership is *over*: the carrier a rule read (`Emission::owns`). So a conditional
            // read excuses a read collision - the two take turns and the ranks decide - while for an
            // **emitted** collision the question is different. Two rules tagging one carrier are separated at
            // runtime only when they also read one, because that is what ownership resolves. A gated rule
            // reading `x` and an ungated one reading `y`, both tagging `shared`, own different carriers, so
            // both emissions survive and nothing downstream tells them apart: the tag is what carrier
            // semantics, identity and ordering all key on.
            //
            // Read-conditionality is therefore *not* an excuse for an emitted collision; shared physical
            // ownership is. That is what makes one dialect's claim on `input.value` coexist with another's
            // reading of it - the same key, resolved by whichever rank comes first.
            let a_reads = consumed_carriers(a);
            let b_reads = consumed_carriers(b);
            // The exemption for a tag collision has to be about the carriers the colliding *emissions*
            // necessarily own, not any carrier either rule might read. A rule reading
            // `first_present: ["first", "second"]` beside one reading `second` overlaps statically and
            // owns `first` at runtime when both are present - so both emissions survive under one tag, which
            // is the defect the exemption was meant to exclude. Same for an unused compose fallback or an
            // unrelated branch leaf.
            let shared_ownership = match (necessarily_owned(a), necessarily_owned(b)) {
                (Some(left), Some(right)) => left == right,
                _ => false,
            };
            // For the **both emit** comparison only: two rules tagging one carrier are separated at runtime
            // when ownership provably resolves them, and read-conditionality is not that proof. The cross
            // comparisons below keep each emission's own condition, because "A emits what B reads" is a
            // question about whether A emits at all.
            let emitted_tags = |rule: &CompiledMessageRule| {
                emitted_patterns(rule)
                    .into_iter()
                    .map(|mut emitted| {
                        emitted.condition = Condition {
                            gate: None,
                            // Ownership resolving them is the only thing that makes a shared tag safe;
                            // nothing else separates two emissions under one name.
                            narrowed: shared_ownership,
                        };
                        emitted
                    })
                    .collect::<Vec<_>>()
            };
            let conflict = [
                (a_reads.clone(), b_reads.clone(), "both read"),
                (emitted_tags(a), emitted_tags(b), "both emit"),
                (
                    emitted_carriers(a),
                    b_reads,
                    "one emits what the other reads",
                ),
                (
                    a_reads,
                    emitted_carriers(b),
                    "one reads what the other emits",
                ),
            ]
            .into_iter()
            // Directional: `a` is the earlier rank, so the question is whether it suppresses `b`.
            .find_map(|(left, right, how)| {
                left.iter()
                    .find_map(|l| {
                        right
                            .iter()
                            .find(|r| {
                                l.condition.suppresses(&r.condition)
                                    && l.pattern.overlaps(&r.pattern)
                            })
                            .map(|_| l.pattern.describe())
                    })
                    .map(|carrier| (carrier, how))
            });
            if let Some((carrier, how)) = conflict {
                return Err(MessageCompileError::ContestedCarrier {
                    first: a.rule_id.clone(),
                    second: b.rule_id.clone(),
                    carrier: format!("{carrier} ({how})"),
                });
            }
        }
    }

    let metadata_candidates = rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| can_emit_metadata(rule))
        .map(|(index, _)| index)
        .collect();
    Ok(MessagePlan {
        rules,
        metadata_candidates,
        raw_forms,
    })
}
