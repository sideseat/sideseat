use super::*;

impl MessagePlan {
    /// Every observation the declared rules find on this span.
    /// Tool definitions this span declares, from every rule that reads a `repr` grammar.
    ///
    /// Separate from `run`, and claimed **per axis**. A tool *definition* is not a message, so a framework
    /// stating its tools on the carrier another rule reads as a conversation is two true statements - which is
    /// why this path is not subject to the message axis's claims. But two rules reading one carrier and both
    /// emitting *definitions* do contend, and nothing resolved that: both survived and their rank became
    /// precedence somewhere downstream, which is a rule id deciding an answer.
    ///
    /// Three arenas, so a definition and a name list read from one carrier both stand while two definition
    /// readings of it do not.
    pub fn tool_definitions<'p>(&'p self, ctx: &MessageContext<'_>) -> Vec<Emission<'p>> {
        // Every rule, filtered to the *metadata* emissions - a definition or a name list. Not gated on the
        // tool-span check: a tool definition is metadata about a span, and the path reading it has always
        // run on every span. Routing by the emission's own target rather than the rule's is what lets one
        // carrier hold both a conversation and the tools it was offered - the conversation goes to `run`,
        // the tools come here, from the same rule.
        let produced = self
            .metadata_candidates
            .iter()
            .flat_map(|&index| emit_rule(&self.rules[index], ctx))
            .filter(|emission| {
                matches!(
                    emission.target,
                    EmitTarget::ToolDefinitions | EmitTarget::ToolNames
                )
            })
            .filter_map(Self::validated_metadata);

        // Claimed by `(carrier, axis)`. The same carrier may yield one definition list and one name list, and
        // one rule legitimately emits several observations from one carrier - so a claim refuses a *different
        // rule* on the *same axis*, exactly as the message path's does.
        let mut claimed: std::collections::HashSet<(OwnedCarrier, EmitTarget)> =
            std::collections::HashSet::new();
        let mut owner: std::collections::HashMap<(OwnedCarrier, EmitTarget), &str> =
            std::collections::HashMap::new();
        let mut kept: Vec<Emission<'p>> = Vec::new();
        for emission in produced {
            let keys: Vec<(OwnedCarrier, EmitTarget)> = emission
                .owns
                .iter()
                .map(|owned| (owned.clone(), emission.target))
                .collect();
            if keys.iter().any(|key| {
                claimed.contains(key)
                    && owner
                        .get(key)
                        .is_some_and(|first| *first != emission.rule_id)
            }) {
                continue;
            }
            for key in keys {
                owner.entry(key.clone()).or_insert(emission.rule_id);
                claimed.insert(key);
            }
            kept.push(emission);
        }
        kept
    }

    /// A metadata emission with its unusable items removed, or `None` where nothing usable is left.
    ///
    /// **Per item**, which is the point. A tool-name list holding `["search", 7]` was persisted as written, and
    /// the read side deserialises the whole column as `Vec<String>` - so the number failed that and took the
    /// valid `"search"` with it. A malformed item must not poison its siblings, and the place to stop it is
    /// where the item is produced.
    ///
    /// **Tool names only**, and that limit is the point rather than an omission. A tool name is a non-blank
    /// string by contract, whatever produced it, so the check is shape-independent and belongs here.
    ///
    /// A tool **definition** is deliberately *not* filtered here, and the reason is measured rather than a
    /// limitation. Asking the question is now possible - the producer shapes are declared
    /// (`rules/vocabulary/tool-shapes.json`), so `extract_tool_name` answers it for any of them, where a hand-written check
    /// over `function.name` once dropped `bedrock/converse`'s perfectly good `get_weather` because at emission
    /// the value is still `{"toolSpec": {…}}`. What is absent is the *harm* this exists to prevent: a tool name
    /// list is read back as `Vec<String>`, so one number failed the whole column, while `tool_definitions` is a
    /// JSON column carried as `Vec<JsonValue>` from persistence to `deduplicate_tools` with no typed
    /// deserialisation anywhere - a malformed item cannot take a sibling with it.
    ///
    /// So the honest place for the definition question is the one place it decides anything:
    /// `deduplicate_tools` keys by name and silently drops what it cannot name, and reports it now.
    fn validated_metadata(emission: Emission<'_>) -> Option<Emission<'_>> {
        if emission.target != EmitTarget::ToolNames {
            return Some(emission);
        }
        let usable = |item: &JsonValue| item.as_str().is_some_and(|name| !name.trim().is_empty());
        let Some(items) = emission.value.as_array() else {
            // A single value rather than a list: the same question, one item.
            return usable(&emission.value).then_some(emission);
        };
        let kept: Vec<JsonValue> = items.iter().filter(|item| usable(item)).cloned().collect();
        let dropped = items.len() - kept.len();
        if dropped > 0 {
            tracing::debug!(
                target: "sideseat::rules",
                rule = %emission.rule_id,
                dropped,
                "tool names were not non-blank strings, so they name nothing"
            );
        }
        if kept.is_empty() {
            return None;
        }
        Some(Emission {
            value: JsonValue::Array(kept),
            ..emission
        })
    }

    /// Every carrier this span carries that a rule reads, read by **one** rule each.
    ///
    /// The first rule to produce from a carrier owns it, and the ranks decide who is first. That is not a
    /// tie-break bolted on: it is what one extractor claiming a carrier meant, and consolidating sixteen
    /// extractors into one entry would otherwise have turned "the earlier one won" into "both emit". Two
    /// dialects really do read the same key - `message` is read by two - and the ownership check permits
    /// the collision precisely because a condition and a rank separate them.
    pub fn run<'p>(&'p self, ctx: &MessageContext<'_>) -> Vec<Emission<'p>> {
        self.stage(ctx, super::schema::MessageStage::Dialect)
    }

    /// What this *event* declares, and what to do with the event's own raw form.
    ///
    /// An event's attributes are read exactly as a span's are - the same envelopes, the same predicates -
    /// because they are the same kind of thing: a flat map a producer wrote. Only where they are found
    /// differs, which is why this is an entry point rather than a new vocabulary.
    pub fn from_event<'p>(
        &'p self,
        event_name: &str,
        event_attrs: &HashMap<String, String>,
        span_name: &str,
        scope_name: Option<&str>,
        span_attrs: &HashMap<String, String>,
        is_tool_span: bool,
    ) -> EventReading<'p> {
        let ctx =
            MessageContext::for_event(span_name, scope_name, span_attrs, event_attrs, is_tool_span);
        let mut out = Vec::new();
        let mut handled = false;
        let mut carrier_present = false;
        let mut owned_attributes: Vec<String> = Vec::new();
        let mut claimed: std::collections::HashSet<OwnedCarrier> = std::collections::HashSet::new();
        // The event's own declaration, asked once. It used to be ORed together from every reading that
        // matched and whose gates held, which meant the policy was stated twice with nothing keeping the
        // two statements consistent - a `true` beside a `false` compiled, and `true` silently won.
        let replaces =
            self.raw_forms.get(event_name) == Some(&super::schema::RawEventForm::Replace);
        for rule in self.rules.iter().filter(|rule| {
            rule.source
                .event_names()
                .iter()
                .any(|name| name == event_name)
        }) {
            // A branch parent states no permission of its own, so the leaves are asked - and `emit_rule` then
            // keeps a forbidden leaf's metadata alone, exactly as on a span.
            if is_tool_span && !reads_tool_spans_anywhere(rule) {
                continue;
            }
            // A rule whose condition fails says nothing about the event, so it must not suppress the raw
            // form either. Asked before `replaces` is set, where it used to be set first.
            if !gates_allow(rule, &ctx) {
                continue;
            }
            // The same routing and ownership as a span: only message emissions, one rule per carrier. An
            // event's attributes are a flat map a producer wrote, so nothing about them earns an exemption
            // from either - and without this a `Claim` became a message and two rules could double-read one
            // of the event's attributes.
            // `Message | Claim` through ownership, as a span does - a claim on an event's attribute means
            // the same thing it means on a span's, and filtering it out beforehand left it with no effect at
            // all. The claims are dropped from the *observations* afterwards, since a claim is not a message.
            let readings: Vec<Emission<'p>> = emit_rule(rule, &ctx)
                .into_iter()
                .filter(|e| matches!(e.target, EmitTarget::Message | EmitTarget::Claim))
                .collect();
            let mut kept = Vec::new();
            keep_unclaimed(readings, &mut claimed, &mut kept);
            // A **claim** counts as handling the event even though it is not a message: that is what a claim
            // means - this payload is framework internals, taken off the table deliberately. Recorded before
            // the message filter below, which drops claims from the observations.
            handled |= !kept.is_empty();
            // Whether the rule's carrier was **there**, asked whatever the reading produced. This is what
            // separates "the container was unreadable" from "the container held nothing this rule wanted",
            // which the two cases below need to answer differently.
            carrier_present |=
                resolve_attribute(&rule.read, ctx.span_attrs).is_some()
                    || rule.read.family.as_deref().is_some_and(|prefix| {
                        ctx.span_attrs.keys().any(|key| key.starts_with(prefix))
                    });
            for owned in kept.iter().flat_map(|e| &e.owns).filter(|o| !o.is_event) {
                if !owned_attributes.contains(&owned.name) {
                    owned_attributes.push(owned.name.clone());
                }
            }
            out.extend(kept.into_iter().filter(|e| e.target == EmitTarget::Message));
        }
        // **Replacement depends on something having read the event**, not on the declaration alone. A
        // container whose declared reads all fail - `gen_ai.input.messages = "{"` on the inference-details
        // event - produced no messages *and* suppressed the raw form, so the event vanished: indistinguishable
        // from it never having been emitted, on the ingest path, with nothing recorded anywhere.
        //
        // Not a declarable policy. Suppressing a container whose payload was *there and unreadable* is a loss
        // with no upside, so there is no second behaviour for an asset to choose between - and a policy member
        // with one sensible value is how a format acquires a setting nobody can reason about.
        //
        // **"Present" is the question, not "read".** A container carrying nothing a rule names is an ordinary
        // empty container, and keeping its raw form would put a message in the feed whose content is whatever
        // unrelated attributes the producer attached - noise a user sees, to protect against a loss that did
        // not happen. A container whose declared carrier *is* present and produced nothing is the malformed
        // case, and there the raw form is the only remaining evidence the payload existed. Distinguishing them
        // properly needs the `Absent | Empty | Malformed | Value` algebra the message path still lacks;
        // carrier presence is the approximation available today, and it is right for both shapes the corpus
        // and the review name.
        let unreadable = replaces && !handled && carrier_present;
        EventReading {
            replaces_raw: replaces && !unreadable,
            unhandled_container: unreadable,
            owned_attributes,
            emissions: out,
        }
    }

    /// The last-resort carriers: the generic input/output pair and the dialect stand-ins for it.
    ///
    /// A stage rather than a rule asking about other rules. *When* it runs is the caller's policy - nothing
    /// recognised the span, or a generation span's answer is still unaccounted for - and that policy is
    /// generic, being a function of the observation type. Which carriers it reads is this plan's business.
    pub fn fallback<'p>(
        &'p self,
        ctx: &MessageContext<'_>,
        already_read: &std::collections::HashSet<OwnedCarrier>,
    ) -> Vec<Emission<'p>> {
        // The carriers a dialect already read, **typed**, because the fallback is not only reached when the
        // dialect stage produced nothing: a generation span whose answer is unaccounted for reads it
        // afterwards. Without this the two stages had independent claim sets, so a dialect claim on
        // `output.value` and the fallback's reading of it both survived.
        //
        // Taken as `Emission::owns` rather than rebuilt from the emitted carrier: a rule with `tag_as` reads
        // one key and reports another, so reconstructing ownership from the report leaves the key it
        // actually read unclaimed - the same defect the `owns` field exists to remove.
        self.stage_with(
            ctx,
            super::schema::MessageStage::Fallback,
            already_read.clone(),
        )
    }

    fn stage<'p>(
        &'p self,
        ctx: &MessageContext<'_>,
        stage: super::schema::MessageStage,
    ) -> Vec<Emission<'p>> {
        self.stage_with(ctx, stage, std::collections::HashSet::new())
    }

    fn stage_with<'p>(
        &'p self,
        ctx: &MessageContext<'_>,
        stage: super::schema::MessageStage,
        mut claimed: std::collections::HashSet<OwnedCarrier>,
    ) -> Vec<Emission<'p>> {
        let mut out = Vec::new();
        for rule in self
            .rules
            .iter()
            .filter(|rule| rule.source == CompiledSource::Span(stage))
        {
            // The tool-span gate is a message-axis question - "may this rule read such a span *as a
            // conversation*" - so it lives here, not in `emit_rule`, which the metadata path also calls.
            //
            // Asked of every rule that could read, which for a branch set is its sub-rules: the parent
            // declares no carrier of its own, so consulting only the parent let a permitted sub-rule be
            // skipped and a forbidden one run.
            if ctx.is_tool_span && !reads_tool_spans_anywhere(rule) {
                continue;
            }
            // Only the message emissions belong to `run`: a definition or a name list is metadata that
            // `tool_definitions` reads, on every span. Filtered by the emission's own target, so one rule
            // reading a carrier that holds both a conversation and a tool list contributes to both paths.
            // Branch-set expansion is inside `emit_rule`, so both paths see the same readings and the
            // primary-empty decision is made on all emissions, not on one axis's slice of them.
            let messages = emit_rule(rule, ctx)
                .into_iter()
                .filter(|e| matches!(e.target, EmitTarget::Message | EmitTarget::Claim));
            keep_unclaimed(messages.collect(), &mut claimed, &mut out);
        }
        out
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub fn rules(&self) -> impl Iterator<Item = &CompiledMessageRule> {
        self.rules.iter()
    }
}

/// Every target this rule could name, wherever the declaration puts it.
///
/// Direct readings, a *fragment's* cases, the extra cases at one selection point, and a branch set's leaves.
/// Asked in one place because two questions depend on it - which axis a rule reads, and whether the metadata
/// path must evaluate it - and a target visible to one but not the other loses an emission on both axes: the
/// message path filters it out and the metadata path never ran the rule.
pub(super) fn possible_targets(rule: &CompiledMessageRule) -> Vec<EmitTarget> {
    if let Some(set) = &rule.branch_set {
        return set
            .primary
            .iter()
            .chain(&set.fallback)
            .chain(&set.always)
            .flat_map(possible_targets)
            .collect();
    }
    let mut out = vec![rule.target];
    for reading in rule
        .alternatives
        .iter()
        .chain(&rule.also)
        .chain(&rule.fallback)
    {
        out.extend(reading.spec.emit);
        // A fragment's cases and this point's extra cases can each override the target.
        out.extend(reading.fragment_cases.iter().filter_map(|case| case.emit));
        out.extend(reading.spec.extra_cases.iter().filter_map(|case| case.emit));
    }
    out
}

/// Whether two rules' relative order is observable.
///
/// Three conditions, and each excludes a pair that legitimately shares a rank:
///
/// - **The same stage.** A fallback-stage rule runs only where the dialect stage produced nothing, so its rank
///   relative to a dialect rule orders nothing.
/// - **An overlapping output axis.** A message and a tool definition are not read by the same path: the
///   metadata path filters messages out and the message path filters metadata out. All five shared ranks in the
///   shipped assets are of this kind.
/// - **The same input domain.** An event rule is selected by event name, so two such rules contend only
///   where their name sets intersect; a span rule reads a span's attributes at one stage.
///
/// The stage comparison is now *inside* the domain question rather than beside it, and that is the defect
/// this function had: two **event** rules declaring different stages compared unequal here - so they were
/// held to be different arenas, where a shared rank is legal - while the event path ignores the stage
/// entirely and ran both, leaving ownership to be decided by comparing their ids.
pub(super) fn share_an_arena(a: &CompiledMessageRule, b: &CompiledMessageRule) -> bool {
    let axis = |rule: &CompiledMessageRule| {
        let targets = possible_targets(rule);
        let message = targets
            .iter()
            .any(|target| matches!(target, EmitTarget::Message | EmitTarget::Claim));
        let metadata = targets
            .iter()
            .any(|target| matches!(target, EmitTarget::ToolDefinitions | EmitTarget::ToolNames));
        (message, metadata)
    };
    let (a_message, a_metadata) = axis(a);
    let (b_message, b_metadata) = axis(b);
    if !((a_message && b_message) || (a_metadata && b_metadata)) {
        return false;
    }
    match (&a.source, &b.source) {
        // Both read a span's attributes - at the same stage, or they never run together.
        (CompiledSource::Span(one), CompiledSource::Span(other)) => one == other,
        // One is selected by event name and the other is not, so they are never candidates together.
        (CompiledSource::Span(_), CompiledSource::Event(_))
        | (CompiledSource::Event(_), CompiledSource::Span(_)) => false,
        // Both are, and they contend wherever an event name is in both sets - **whatever stage they
        // declare**, because an event rule has no stage to declare.
        (CompiledSource::Event(one), CompiledSource::Event(other)) => {
            one.iter().any(|name| other.contains(name))
        }
    }
}

/// Which rules the metadata path needs to evaluate at all.
///
/// A rule qualifies if it, or any of its readings, or any sub-rule of its branch set, can name a metadata
/// target. Also read by the tool-span gate, which must exempt metadata: a tool definition is not a reading
/// of the span's conversation, so it is not what that gate is about.
pub(super) fn can_emit_metadata(rule: &CompiledMessageRule) -> bool {
    if let Some(set) = &rule.branch_set {
        return set
            .primary
            .iter()
            .chain(&set.fallback)
            .chain(&set.always)
            .any(can_emit_metadata);
    }
    rule.tool_repr.is_some()
        || possible_targets(rule)
            .into_iter()
            .any(|target| matches!(target, EmitTarget::ToolDefinitions | EmitTarget::ToolNames))
}

/// Whether this rule, or any sub-rule of its branch set, may read a tool span as a conversation.
///
/// A branch-set parent declares no carrier, so its own flag says nothing about what its sub-rules read.
pub(super) fn reads_tool_spans_anywhere(rule: &CompiledMessageRule) -> bool {
    match &rule.branch_set {
        Some(set) => set
            .primary
            .iter()
            .chain(&set.fallback)
            .chain(&set.always)
            .any(reads_tool_spans_anywhere),
        None => rule.reads_tool_spans,
    }
}

/// Keep one rule's emissions, unless a rule before it already owns their carrier.
///
/// Per *rule*, not per emission: one rule legitimately emits many observations from one carrier - a list of
/// turns is one carrier and many messages - so a rule that owns a carrier keeps everything it read from it,
/// and the next rule reading that carrier keeps nothing.
pub(super) fn keep_unclaimed<'p>(
    produced: Vec<Emission<'p>>,
    claimed: &mut std::collections::HashSet<OwnedCarrier>,
    out: &mut Vec<Emission<'p>>,
) {
    // Claimed within this batch as well as before it. A branch set's sub-rules are separate rules whose
    // emissions arrive flattened into one vector, so two of them reading one carrier would both survive a
    // check that only consulted earlier *top-level* claims.
    //
    // Per rule still, not per emission: one rule legitimately emits many observations from one carrier, so a
    // carrier this batch has already claimed is only refused to a *different* rule within it.
    let mut mine: std::collections::HashSet<OwnedCarrier> = std::collections::HashSet::new();
    let mut owner: std::collections::HashMap<OwnedCarrier, &str> = std::collections::HashMap::new();
    for emission in produced {
        // A tool definition is metadata about the span, not a reading of its conversation, so it neither
        // claims a carrier nor is blocked by one: a dialect legitimately states its tools on a carrier
        // another rule reads as a conversation, and both statements are true.
        if emission.target == EmitTarget::ToolDefinitions {
            out.push(emission);
            continue;
        }
        // Owned before this batch, or by a different rule within it.
        let rule = emission.rule_id;
        let taken = |owned: &OwnedCarrier| {
            claimed.contains(owned) || owner.get(owned).is_some_and(|first| *first != rule)
        };
        let emission = if emission.owns.iter().any(&taken) {
            match given_way(emission, &taken) {
                Some(kept) => kept,
                None => continue,
            }
        } else {
            emission
        };
        for owned in &emission.owns {
            owner.insert(owned.clone(), emission.rule_id);
            mine.insert(owned.clone());
        }
        out.push(emission);
    }
    claimed.extend(mine);
}

/// An emission without the compose members another rule's carriers took, or `None` where it cannot give way.
///
/// Only a member a compose read through its conditional **fallback** gives way: the key is not the dialect's
/// own, so its owner has the better claim. Anything else taken drops the reading whole, as before. What is
/// left is pruned in place - member order kept, nothing re-read - and is kept only while it is still a message:
/// a named member the compose read survives, and the compose's `where` still holds. It then owns what is left,
/// and the carrier it gave up stays its owner's. A branch set's choice of fallback leaves is made before
/// ownership and is not revisited.
fn given_way<'p>(
    mut emission: Emission<'p>,
    taken: &dyn Fn(&OwnedCarrier) -> bool,
) -> Option<Emission<'p>> {
    let blocked: Vec<OwnedCarrier> = emission
        .owns
        .iter()
        .filter(|owned| taken(owned))
        .cloned()
        .collect();
    let Emission {
        value,
        yields,
        owns,
        ..
    } = &mut emission;
    if !blocked
        .iter()
        .all(|carrier| yields.iter().any(|yielded| &yielded.carrier == carrier))
    {
        return None;
    }
    let compose = yields.first()?.compose;
    let JsonValue::Object(object) = value else {
        return None;
    };
    for yielded in yields.iter().filter(|y| blocked.contains(&y.carrier)) {
        object.shift_remove(yielded.member);
    }
    let still_a_message = compose
        .members
        .iter()
        .filter_map(|member| member.spec.as_member.as_deref())
        .any(|name| object.contains_key(name));
    if !still_a_message || !predicates_hold(value, &compose.require) {
        return None;
    }
    owns.retain(|owned| !blocked.contains(owned));
    yields.retain(|yielded| !blocked.contains(&yielded.carrier));
    Some(emission)
}
