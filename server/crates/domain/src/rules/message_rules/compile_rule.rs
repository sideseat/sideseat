use super::*;

/// Compile one rule, with every validation the engine performs.
///
/// Extracted so a branch set's sub-readings get exactly the same checks as a top-level rule - a sub-reading
/// that skipped them would be the one place a no-op could still hide.
pub(super) fn compile_rule(
    file_id: &str,
    rule: &MessageRule,
    fragments: &HashMap<String, Vec<Alternative>>,
) -> Result<CompiledMessageRule, MessageCompileError> {
    let MessageRule {
        id,
        doc,
        source,
        tool_repr,
        read,
        compose,
        parse,
        wrap,
        emit,
        aggregate_into_array,
        alternatives,
        also,
        fallback,
        require_members,
        require_non_empty,
        require_non_blank,
        branch_set,
        elements,
        walk,
        sections,
        reads_tool_spans,
        tag_as,
        unless,
        when,
        instrumentation_scope,
        legacy_rank,
    } = rule;
    // The values every check below reads, resolved once. The *declarations* above keep their presence, which
    // is a separate question and is asked only by the branch-seam refusals: a field written out with its
    // default value is still a statement the engine does not read where it was written.
    let emit_target = emit.unwrap_or(EmitTarget::Message);
    let aggregate = aggregate_into_array.unwrap_or(false);
    let non_empty = require_non_empty.unwrap_or(false);
    let non_blank = require_non_blank.unwrap_or(false);
    let tool_spans = reads_tool_spans.unwrap_or(false);
    if instrumentation_scope.as_ref().is_some_and(|scope| {
        scope.name.is_empty() || scope.version_prefix.as_ref().is_some_and(String::is_empty)
    }) {
        return Err(MessageCompileError::Inexpressible {
            rule: id.clone(),
            detail: "an instrumentation scope name or version prefix is empty, which would match no \
                     meaningful producer scope",
        });
    }
    if compose.is_none() && branch_set.is_none() && read.named_count() != 1 {
        return Err(MessageCompileError::NotExactlyOneCarrier { rule: id.clone() });
    }
    // Every combination the runner would silently ignore is refused here instead. Each of these
    // was accepted and did nothing, which reads as a rule that works.
    let inexpressible = |detail: &'static str| MessageCompileError::Inexpressible {
        rule: id.clone(),
        detail,
    };
    // Every gate a message rule can carry, through **one** validator. Three call sites grew the checks
    // separately - the detection compiler, the field-source compiler, and this one - so the same declaration was
    // refused in two places and compiled in a third. A compose member's conditional fallback is a gate too, read
    // only where it holds, so an undeclarable one makes that member's last resort dead while reading as though
    // it has one.
    let gates = [when.as_ref(), unless.as_ref()]
        .into_iter()
        .flatten()
        .chain(
            compose
                .iter()
                .flat_map(|compose| &compose.members)
                .filter_map(|member| member.fallback.as_ref())
                .map(|fallback| &fallback.when),
        );
    for gate in gates {
        if let Some(detail) = message_gate_defect(gate) {
            return Err(MessageCompileError::Inexpressible {
                rule: id.clone(),
                detail,
            });
        }
    }
    if compose.is_some()
        && (wrap.is_some()
            || sections.is_some()
            || !alternatives.is_empty()
            || !also.is_empty()
            || !fallback.is_empty()
            || parse.is_some()
            // `tag_as` is the dangerous one: a compose always emits `compose.tag`, while the static
            // conflict analysis models `tag_as` - so a compose declaring both emitted one carrier and was
            // checked for collisions against another. The rest are ignored the same way the list above is.
            || tag_as.is_some()
            || aggregate_into_array.is_some()
            || elements.is_some()
            || walk.is_some()
            || require_non_empty.is_some()
            || require_non_blank.is_some()
            || require_members.is_some())
    {
        return Err(inexpressible(
            "`compose` builds the whole message and tags it with its own `tag`, so `wrap`, \
                 `sections`, `alternatives`, `parse`, `tag_as` and the reading requirements would be \
                 ignored",
        ));
    }
    // `each` reads *every* listed key the span carries; only `tool_repr` iterates its carriers. Declared
    // anywhere else it would be silently read as `first_present` - which is precisely the conflation the two
    // members exist to end, so the wrong pairing is a refusal rather than a quiet reinterpretation.
    if !read.each.is_empty() && tool_repr.is_none() {
        return Err(inexpressible(
            "`each` reads every listed key as its own observation, and only `tool_repr` iterates its \
                 carriers - elsewhere it would be read as `first_present`, which is the ambiguity the two \
                 members replace. Use `first_present` for ordered alternatives",
        ));
    }
    // A repeated key can never mean what it says: under `first_present` the second occurrence is
    // unreachable, and under `each` it would read one attribute as two observations of it. An *empty* list
    // needs no rule of its own - it names no carrier, so `named_count` already refuses a rule whose only
    // source it is. A **single**-key list is deliberately allowed: `single_carrier_of` reads it as the exact
    // carrier it is, which is what lets a renamed key be declared alongside nothing else.
    for keys in [&read.first_present, &read.each] {
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        if keys.iter().any(|key| !seen.insert(key.as_str())) {
            return Err(inexpressible(
                "a carrier list names one key twice - under `first_present` the repeat is unreachable, and \
                 under `each` it would read one attribute as two observations",
            ));
        }
    }
    // **A closed role map must say what an unmapped value means.** Closedness is enforced - an unlisted value
    // is discarded, which is the point - but with no literal fallback the message is emitted with **no role**,
    // and normalisation then infers one from unrelated payload members: Assistant if the message happens to
    // carry tool calls, User otherwise. So an unknown speaker's meaning came from whether the turn used a tool.
    //
    // The fallback is `wrap.role`, which every shipped closed map already declares - so this is a gate rather
    // than a migration. Reporting the unmapped *value* is the other half and waits for the outcome algebra: it
    // wants a recovered role carrying an unknown-role defect, which is exactly the shape a sum type cannot hold.
    if let Some(wrap) = wrap
        && wrap.role_map_is_closed
        && wrap.role.is_none()
    {
        return Err(inexpressible(
            "closes its `role_map` and states no `role` to fall back on - an unmapped value then leaves the \
                 message with no role at all, and normalisation infers one from unrelated members of the \
                 payload",
        ));
    }
    // **A role a rule states must be a role.** `role`, every `role_map` output, and `trailing`'s role literal
    // are compared against the vocabulary - `role_map: {"model": "assisstant"}` compiled, and the typo became
    // *User*, because an unrecognised role folds to User rather than being refused. So a rule could say
    // "assistant" and mean "user", and nothing anywhere said otherwise.
    //
    // Compared against `ChatRole::try_from_str`, which is the same question every reader asks - not a second
    // list, which would drift from it.
    {
        let known = |role: &str| crate::sideml::ChatRole::try_from_str(role).is_some();
        let mut stated: Vec<&String> = Vec::new();
        if let Some(wrap) = wrap {
            stated.extend(wrap.role.as_ref());
            stated.extend(wrap.role_map.values());
        }
        if stated.iter().any(|role| !known(role)) {
            return Err(inexpressible(
                "states a role that is not one - an unrecognised role folds to `user`, so the declaration \
                     would silently mean something other than it says",
            ));
        }
        // A compose's trailing literals are JSON values, so the role is read where it is a string. A
        // non-string `role` literal is a different defect and is not this check's business.
        if let Some(compose) = compose
            && compose.trailing.iter().any(|(member, value)| {
                member == "role" && value.as_str().is_some_and(|role| !known(role))
            })
        {
            return Err(inexpressible(
                "states a trailing role that is not one - it would fold to `user`",
            ));
        }
    }
    // **Two declarations must not write the same output member.** A wrap builds its object by inserting in a
    // fixed order - role, literal members, pre-content attachments, the content, post-content attachments -
    // and every insert *overwrites*. So `{"role": "user", "members": {"role": "assistant"},
    // "content_as": "role"}` compiled and produced a message whose role is its content, with the two
    // declarations before it silently discarded. Attachments could overwrite literals, the content and each
    // other, and `compose.trailing` could overwrite a named or swept member.
    //
    // Refused rather than ordered, because the order is not the point: two declarations writing one name is
    // a rule that states two things about one member, and only one of them is true. If replacement is ever
    // wanted it needs a name of its own, not an insertion order a reader has to know.
    if let Some(wrap) = wrap {
        let mut names: Vec<String> = Vec::new();
        if wrap.role.is_some() || wrap.role_from.is_some() {
            names.push("role".to_string());
        }
        names.extend(wrap.members.keys().cloned());
        // The content member is written unless a tool-call list replaces it, which is stated at that branch.
        if wrap.tool_calls_from.is_none() {
            names.push(
                wrap.content_as
                    .clone()
                    .unwrap_or_else(|| "content".to_string()),
            );
        }
        names.extend(wrap.attach.iter().map(|a| a.as_member.clone()));
        if let Some(spec) = &wrap.tool_calls_from {
            names.push(
                spec.as_member
                    .clone()
                    .unwrap_or_else(|| "tool_calls".to_string()),
            );
        }
        if let Some(spec) = &wrap.tool_call_from {
            names.push(
                spec.as_member
                    .clone()
                    .unwrap_or_else(|| "tool_call".to_string()),
            );
        }
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        if names.iter().any(|name| !seen.insert(name.as_str())) {
            return Err(inexpressible(
                "two declarations of one envelope write the same output member, so one of them is silently \
                     discarded - a rule stating two things about one member states one thing that is false",
            ));
        }
    }
    // The same question for a compose: its `trailing` literals are inserted after the members, so a trailing
    // name that a member also writes overwrites it. A **sweep** member's names are only known at read time, so
    // the sweep has to exclude every fixed output name - which is a declaration it can make (`except`) and
    // which `vercel-ai.response` did not, leaving `ai.response.role` swept and then overwritten by
    // `trailing.role`.
    if let Some(compose) = compose {
        let named: Vec<&str> = compose
            .members
            .iter()
            .filter_map(|member| member.as_member.as_deref())
            .collect();
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        if named.iter().any(|name| !seen.insert(*name)) {
            return Err(inexpressible(
                "two compose members write the same output member, so one is silently discarded",
            ));
        }
        for trailing in compose.trailing.keys() {
            if seen.contains(trailing.as_str()) {
                return Err(inexpressible(
                    "a `trailing` literal writes an output member a compose member also writes, and \
                         trailing is inserted last - so the member's value is silently discarded",
                ));
            }
        }
        for member in &compose.members {
            let Some(prefix) = &member.sweep_prefix else {
                continue;
            };
            // A swept name is `<prefix><rest>`, so the fixed names it could collide with are the ones whose
            // spelling a producer could write under that prefix.
            for fixed in seen
                .iter()
                .copied()
                .chain(compose.trailing.keys().map(String::as_str))
            {
                if !member.except.iter().any(|except| except == fixed) {
                    return Err(inexpressible(
                        "a sweep member does not exclude an output member the rule also writes, so whichever \
                             is inserted last silently discards the other - add it to `except`",
                    ));
                }
                let _ = prefix;
            }
        }
    }
    // **`parse` is required wherever the reading parses a raw scalar of its own.** Omitted, it meant three
    // incompatible things depending on what sat beside it: text for a compose, JSON for an ordinary read or a
    // `tool_repr`, JSON-or-string for a named family. So one absent declaration was three different decisions,
    // and which one applied was a property of a *sibling* member - the same defect `attribute_any_of` had.
    //
    // The exemptions are stated rather than implicit, and each is a reading that parses no scalar itself:
    //
    // - a **compose** reads through its members, each of which declares its own mode;
    // - a **sweep** member takes whatever a prefix holds, so sniffing is what it is for;
    // - an **indexed family** assembles entries from keys rather than parsing one string.
    //
    // Corpus-neutral: every shipped reading that parses a scalar already declares it, which is what makes this
    // a gate rather than a migration.
    let parses_a_scalar = read.attribute.is_some()
        || !read.first_present.is_empty()
        || !read.each.is_empty()
        || read.attribute_family.is_some();
    if parse.is_none() && compose.is_none() && parses_a_scalar {
        return Err(inexpressible(
            "reads a raw attribute and does not declare `parse`, which means text, JSON or JSON-or-string \
                 depending on what sits beside it - state the mode",
        ));
    }
    if let Some(compose) = compose {
        for member in &compose.members {
            // A member naming carriers reads one of their strings; a sweep member takes whatever a prefix
            // holds, which is the one place sniffing is the point.
            if member.parse.is_none() && !member.from_any_of.is_empty() {
                return Err(inexpressible(
                    "a compose member names carriers and does not declare `parse` - each member reads its \
                         own string, so the mode is the member's",
                ));
            }
        }
    }
    // `require_non_empty` / `require_non_blank` ask about a **raw carrier string**, and an indexed family has
    // none: its entries are assembled from many keys, so there is nothing for the check to be about. The branch
    // reading a family returns before these checks run, so such a declaration was read from nowhere - refused
    // rather than silently ignored, because the asset would otherwise state a filter it does not have.
    // (`require_members` is the entry-level filter that family reads *do* honour.)
    if read.indexed_family.is_some() && (non_empty || non_blank) {
        return Err(inexpressible(
            "an indexed family assembles each entry from several keys, so there is no raw string for \
                 `require_non_empty` or `require_non_blank` to ask about - use `require_members`",
        ));
    }
    // A wrap is meaningful on an *aggregated* family: the entries become one array, and one array needs an
    // envelope saying what it is - a result set is one observation, not one message per document.
    if read.indexed_family.is_some()
        && ((wrap.is_some() && !aggregate) || !alternatives.is_empty() || !also.is_empty())
    {
        return Err(inexpressible(
            "an indexed family assembles each entry itself, so `wrap` and `alternatives` would \
                 be ignored",
        ));
    }
    // `elements` reads an array element by element and derives each element's own tag, so a `tag_as` beside
    // it is a name the rule never emits - and the static conflict analysis modelled that name while the
    // runtime emitted the derived one. The rest of the list is what the elements branch returns before
    // reaching: it parses the carrier itself and emits each element, with no envelope and no readings.
    if elements.is_some()
        && (tag_as.is_some()
            || wrap.is_some()
            || sections.is_some()
            || walk.is_some()
            || aggregate_into_array.is_some()
            || require_members.is_some()
            || !alternatives.is_empty()
            || !also.is_empty()
            || !fallback.is_empty())
    {
        return Err(inexpressible(
            "`elements` derives each element's own tag and emits it directly, so `tag_as`, an envelope, \
                 a walk, an aggregate or a reading would be ignored",
        ));
    }
    // `sections` returns before the walk, the aggregate, the readings and the fallback, so each of those was
    // accepted and dead - the list was incomplete rather than absent, which is the harder kind to notice.
    if sections.is_some()
        && (wrap.is_some()
            || !alternatives.is_empty()
            || !also.is_empty()
            || !fallback.is_empty()
            || walk.is_some()
            || aggregate_into_array.is_some()
            || tag_as.is_some())
    {
        return Err(inexpressible(
            "`sections` builds each section's message and emits it directly, so `wrap`, `alternatives`, a \
                 `fallback`, a walk, an aggregate or a `tag_as` would be ignored",
        ));
    }
    // **An aggregate wraps the array once, so a per-reading envelope beside it is dead.** The runtime built
    // one observation from the entries and discarded every reading's envelope *and* the rule's - while the
    // indexed-family aggregate applies the rule's envelope once, so identical syntax meant different things by
    // read form. The rule's envelope now applies to the assembled array in both, and a per-reading envelope is
    // refused: "construct each, then aggregate" is a different operation and nothing declares it.
    if *aggregate_into_array == Some(true) {
        let declares_wrap = |readings: &[Alternative]| {
            readings.iter().any(|reading| {
                reading.wrap.is_some() || reading.extra_cases.iter().any(|c| c.wrap.is_some())
            })
        };
        if declares_wrap(alternatives) || declares_wrap(also) || declares_wrap(fallback) {
            return Err(inexpressible(
                "an aggregate builds one observation from every reading, so a per-reading envelope would be \
                     discarded - declare the envelope on the rule, which wraps the assembled array",
            ));
        }
    }
    // **A `tool_repr` states literals that must be able to match something.** The typed structure accepted
    // every one of these and each is a declaration that cannot mean what it says:
    //
    // | Declared | What it does |
    // | --- | --- |
    // | no candidates | the entries are found and nothing is tried as a tool |
    // | an empty field, label or marker | matches at position zero of every string, so every entry is a repr |
    // | a case-folded duplicate source type | the map is compared case-insensitively, so the second is unreachable |
    // | a target or default outside JSON Schema's primitives | a `type` member no validator acts on |
    if let Some(repr) = tool_repr {
        if repr.candidates.is_empty() {
            return Err(inexpressible(
                "declares a `tool_repr` with no candidates, so the entries are found and nothing is tried as \
                     a tool",
            ));
        }
        let literals = [
            ("name_field", &repr.name_field),
            ("description_field", &repr.description_field),
            ("name_label", &repr.name_label),
            ("description_label", &repr.description_label),
            ("arguments_label", &repr.arguments_label),
        ];
        if literals.iter().any(|(_, value)| value.trim().is_empty())
            || repr.repr_markers.iter().any(|m| m.trim().is_empty())
            || repr.parameter_members.iter().any(|m| m.trim().is_empty())
            || repr.field_terminators.iter().any(|t| t.trim().is_empty())
            || repr
                .type_map
                .iter()
                .any(|(source, target)| source.trim().is_empty() || target.trim().is_empty())
        {
            return Err(inexpressible(
                "declares an empty field, label, marker or type name in its `tool_repr` - an empty token \
                     matches at position zero of every string, so it matches everything",
            ));
        }
        let mut folded: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        if repr
            .type_map
            .iter()
            .any(|(source, _)| !folded.insert(source.to_lowercase()))
        {
            return Err(inexpressible(
                "maps one source type twice in its `tool_repr`; the map is compared case-insensitively, so \
                     the second mapping is unreachable",
            ));
        }
        let primitive = |name: &String| {
            super::schema::UnknownType::PRIMITIVES
                .iter()
                .any(|allowed| allowed == name)
        };
        let default_target = match &repr.type_default {
            super::schema::UnknownType::Unconstrained => None,
            super::schema::UnknownType::MapTo(target) => Some(target),
        };
        if repr.type_map.iter().any(|(_, target)| !primitive(target))
            || default_target.is_some_and(|target| !primitive(target))
        {
            return Err(inexpressible(
                "maps a type to something that is not a JSON Schema primitive, which is a `type` member no \
                     reader acts on",
            ));
        }
    }
    // **A constructor and its target have to be about the same thing.** `EmitTarget` is a filing destination
    // and nothing checked that the reading filed there was the shape that destination holds, so
    // `{"wrap": {"role": "user"}, "emit": "tool_names"}` compiled and filed `{"role":"user","content":"hello"}`
    // as a tool *name*.
    //
    // Not a full typing of the constructor/target pairs - that belongs to the discriminated grammar, and
    // pretending this matrix is it would be worse than the gap. These are the pairs that are provably
    // incoherent today:
    //
    // | Constructor | Requires | Why |
    // | --- | --- | --- |
    // | `wrap`, `sections`, `elements` | a message target | each builds a message: a role, a content member, blocks |
    // | `tool_repr` | `tool_definitions` | it reads a language's `repr` of tool schemas |
    // | any constructor | not `claim` | a claim is recognition - it takes a payload off the table and emits nothing, so a constructed value is discarded |
    let targets = {
        let mut targets = vec![emit.unwrap_or(EmitTarget::Message)];
        for reading in alternatives
            .iter()
            .chain(also.iter())
            .chain(fallback.iter())
        {
            targets.extend(reading.emit);
        }
        targets
    };
    let builds_a_message = wrap.is_some() || sections.is_some() || elements.is_some();
    if builds_a_message && !targets.contains(&EmitTarget::Message) {
        return Err(inexpressible(
            "builds a message - an envelope, sections or element passes - and files it under a target that \
                 does not hold messages",
        ));
    }
    if tool_repr.is_some() && !targets.contains(&EmitTarget::ToolDefinitions) {
        return Err(inexpressible(
            "reads a language's `repr` of tool schemas and does not file them as tool definitions",
        ));
    }
    if targets == vec![EmitTarget::Claim]
        && (builds_a_message
            || compose.is_some()
            || tool_repr.is_some()
            || *aggregate_into_array == Some(true))
    {
        return Err(inexpressible(
            "constructs a value and emits a `claim`, which takes a payload off the table and emits nothing - \
                 so whatever was built is discarded",
        ));
    }
    // **A walk's `stop_on` names clauses of its own rule.** A name that matches nothing can never stop the
    // descent, so the walk silently runs to `max_depth` - and an id that is merely misspelled looks exactly like
    // a deliberate "never stop".
    if let Some(walk) = walk {
        let local: std::collections::BTreeSet<&str> = alternatives
            .iter()
            .chain(also.iter())
            .chain(fallback.iter())
            .map(|reading| reading.id.as_str())
            .collect();
        if walk.stop_on.iter().any(|id| !local.contains(id.as_str())) {
            return Err(inexpressible(
                "a walk's `stop_on` names a clause this rule does not declare, so it could never stop the \
                     descent - which is indistinguishable from meaning never to stop",
            ));
        }
    }
    // **An element pass states exactly one action, and its decision table is total.** Five shapes were
    // accepted and each did something other than what it said:
    //
    // | Shape | What happened |
    // | --- | --- |
    // | no passes at all | the rule reads its array and emits nothing |
    // | a pass with neither `tag_from` nor `group` | the pass matches elements and drops them |
    // | a pass with **both** | `group` silently wins and `tag_from` is dead |
    // | an empty `group.by` | no element derives a key, so every run is empty |
    // | a derived value missing from `tag_by_key` | the whole run is dropped, silently |
    //
    // The last is the one to notice: a decision table that answers for an element and then has no tag for the
    // answer discards content the rule matched *on purpose*. Logfire's data already satisfies all five.
    if let Some(elements) = elements {
        if elements.passes.is_empty() {
            return Err(inexpressible(
                "`elements` declares no passes, so the rule reads its array and emits nothing",
            ));
        }
        for pass in &elements.passes {
            match (&pass.tag_from, &pass.group) {
                (None, None) => {
                    return Err(inexpressible(
                        "an element pass declares neither `tag_from` nor `group`, so it matches elements and \
                             drops them",
                    ));
                }
                (Some(_), Some(_)) => {
                    return Err(inexpressible(
                        "an element pass declares both `tag_from` and `group`; `group` wins and `tag_from` \
                             would be ignored",
                    ));
                }
                _ => {}
            }
            let Some(group) = &pass.group else { continue };
            if group.by.is_empty() {
                return Err(inexpressible(
                    "a grouped element pass declares no cases, so no element derives a key and every run is \
                         empty",
                ));
            }
            for case in &group.by {
                if !group.tag_by_key.contains_key(&case.value) {
                    return Err(inexpressible(
                        "a grouped element pass derives a value its `tag_by_key` has no tag for, so a run \
                             the rule matched on purpose is silently discarded",
                    ));
                }
            }
        }
    }
    // The same for an indexed family, whose branch also returns before the walk, the readings and the
    // fallback. Its existing refusal covered `wrap`, `alternatives` and `also` only.
    if read.indexed_family.is_some()
        && (!fallback.is_empty() || walk.is_some() || sections.is_some())
    {
        return Err(inexpressible(
            "an indexed family assembles each entry itself, so a `fallback`, a walk or `sections` would be \
                 ignored",
        ));
    }
    // And a named family, which is the newest of the three and returns at the same point.
    if read.attribute_family.is_some()
        && (!alternatives.is_empty()
            || !also.is_empty()
            || !fallback.is_empty()
            || walk.is_some()
            || sections.is_some()
            || elements.is_some()
            || aggregate_into_array.is_some())
    {
        return Err(inexpressible(
            "a named family emits one observation per member, so `alternatives`, a `fallback`, a walk, \
                 `sections`, `elements` or an aggregate would be ignored",
        ));
    }
    if read.entry_member.is_some() && read.indexed_family.is_none() {
        return Err(inexpressible(
            "`entry_member` is a sub-level of an indexed entry and means nothing without \
                 `indexed_family`",
        ));
    }
    if require_members.is_some() && read.indexed_family.is_none() {
        return Err(inexpressible(
            "`require_members` is checked per indexed entry and means nothing without \
                 `indexed_family`",
        ));
    }
    if read.entry_require.is_some() && read.indexed_family.is_none() {
        return Err(inexpressible(
            "`entry_require` is checked against each assembled indexed entry and means nothing without \
                 `indexed_family`",
        ));
    }
    if read
        .entry_require
        .as_ref()
        .is_some_and(PredicateSet::is_empty)
    {
        return Err(inexpressible(
            "`entry_require` is declared with no predicate, which keeps every entry - leave it out to \
                 require nothing",
        ));
    }
    // The requirement's own literals, which nothing checked. An empty member name makes the evaluator look
    // for `<entry>.` or `<entry>..`, so the rule is dead - and an explicitly empty group holds
    // unconditionally, which is the opposite of "at least one of these".
    if let Some(members) = require_members {
        if members
            .all_of
            .iter()
            .chain(&members.any_of)
            .any(|requirement| requirement.name.is_empty())
        {
            return Err(inexpressible(
                "a `require_members` entry names an empty member, so it asks for `<entry>.` and can \
                 never hold",
            ));
        }
        if members.all_of.is_empty() && members.any_of.is_empty() {
            return Err(inexpressible(
                "`require_members` is declared with no requirement, which holds for every entry - leave \
                 it out to require nothing",
            ));
        }
    }
    // Combinations the evaluator silently ignores. Each of these compiled and did nothing, which is worse
    // than a refusal: the rule reads as a statement the engine never makes.
    if read.entry_value.is_none() && read.entry_value_parse.is_some() {
        return Err(inexpressible(
            "`entry_value_parse` says how the *projected* value is read, so it means nothing without \
                 `entry_value`",
        ));
    }
    if read.indexed_family.is_none()
        && (read.overlay.is_some()
            || !read.numeric_members.is_empty()
            || read.entry_value.is_some()
            || read.entry_value_parse.is_some())
    {
        return Err(inexpressible(
            "`overlay`, `numeric_members` and `entry_value` describe an indexed family's entries and \
                 are read only for one",
        ));
    }
    if aggregate
        && alternatives
            .iter()
            .chain(also)
            .chain(fallback)
            .any(|a| a.emit.is_some())
    {
        return Err(inexpressible(
            "an aggregate is one observation, so its target is the rule's; a per-reading `emit` beside \
                 `aggregate_into_array` would be ignored",
        ));
    }
    // `only_plain_data` answers "is this already a message" and returns the value untouched when it is, so
    // anything that would have built a block or a call list is skipped. Silently: the declaration reads as
    // though the block is built. Refused rather than reordered, because the two say contradictory things -
    // one that the value may already be a message, the other that it is a payload to wrap.
    if let Some(wrap) = wrap
        && wrap.only_plain_data
        && (wrap.block.is_some()
            || wrap.prepend_block.is_some()
            || wrap.tool_calls_from.is_some()
            || wrap.tool_call_from.is_some())
    {
        return Err(inexpressible(
            "`only_plain_data` passes an already-message-shaped value through untouched, so a block or a \
                 tool-call constructor beside it would be skipped for exactly the values it exists to \
                 recognise",
        ));
    }
    // A canonical tool definition is a tool definition. Emitted on the message axis it would be a message
    // shaped like one, which no reader expects.
    if compose.as_ref().is_some_and(|c| c.as_tool_definition)
        && emit_target != EmitTarget::ToolDefinitions
    {
        return Err(inexpressible(
            "`as_tool_definition` builds a canonical tool definition, so the rule's target must be \
                 `tool_definitions`",
        ));
    }
    if let Some(overlay) = &read.overlay {
        if overlay.from.is_empty()
            || overlay.when_member_prefix.is_empty()
            || overlay.as_member.is_empty()
        {
            return Err(inexpressible(
                "an overlay names a carrier, the flattened members it replaces and the member they \
                     become; an empty one of those is not a name - and an empty prefix matches every \
                     member, so the overlay would delete the whole entry",
            ));
        }
        if overlay.select_any_of.is_empty() || overlay.content_any_of.is_empty() {
            return Err(inexpressible(
                "an overlay with no path to its counterpart list, or none to that counterpart's \
                     content, can never find anything",
            ));
        }
    }
    if tool_repr.is_some()
        && (emit_target != EmitTarget::ToolDefinitions
            || read.indexed_family.is_some()
            || aggregate
            || non_empty
            || non_blank
            || wrap.is_some()
            || compose.is_some()
            || sections.is_some()
            || elements.is_some()
            || walk.is_some()
            || branch_set.is_some()
            || !alternatives.is_empty()
            || !also.is_empty()
            || !fallback.is_empty())
    {
        return Err(inexpressible(
            "a `repr` grammar assembles tool definitions itself from attribute carriers, so an indexed \
                 family, an aggregate, a content requirement, a reading, an envelope, or any target but \
                 `tool_definitions` would be ignored",
        ));
    }
    // `alternatives` and `also` may coexist: the first list is the ordered question "which shape is
    // this", the second is "and read this as well, always". One carrier really does need both - a dialect's
    // response member holds the reply *and* the inner turns that produced it - and refusing the pair forced
    // that into two rules claiming one carrier, which the ownership check rightly refuses.
    if let Some(sections) = sections {
        if sections.split_on.is_empty() {
            return Err(inexpressible("`sections.split_on` is empty"));
        }
        // A default route consumes every section, so anything after it is dead.
        if let Some(position) = sections
            .routes
            .iter()
            .position(|route| route.tag_prefix.is_none())
            && position + 1 < sections.routes.len()
        {
            return Err(inexpressible(
                "a route with no `tag_prefix` claims every section, so the routes after it can \
                     never match",
            ));
        }
    }
    if let Some(compose) = compose {
        for member in &compose.members {
            if member.sweep_prefix.is_some()
                && (member.as_member.is_some() || !member.from_any_of.is_empty())
            {
                return Err(inexpressible(
                    "a compose member is either a sweep or a named source, not both - the sweep \
                         would silently win",
                ));
            }
            if member.sweep_prefix.as_deref() == Some("") {
                return Err(inexpressible(
                    "a compose member has an empty `sweep_prefix`",
                ));
            }
            if member.sweep_prefix.is_none() && member.as_member.is_none() {
                return Err(inexpressible(
                    "a compose member names neither a member nor a sweep",
                ));
            }
        }
    }
    if let Some(compose) = compose {
        if read.named_count() != 0 {
            return Err(MessageCompileError::NotExactlyOneCarrier { rule: id.clone() });
        }
        if compose.tag.is_empty() || compose.members.is_empty() {
            return Err(MessageCompileError::EmptyCarrier { rule: id.clone() });
        }
    } else {
        // Every name the rule could read or tag with must be non-empty: an empty prefix
        // matches every attribute of every span.
        let named = [
            read.attribute.as_deref(),
            read.indexed_family.as_deref(),
            tag_as.as_deref(),
        ];
        if named.iter().flatten().any(|name| name.is_empty())
            || read.first_present.iter().any(String::is_empty)
            || read.each.iter().any(String::is_empty)
        {
            return Err(MessageCompileError::EmptyCarrier { rule: id.clone() });
        }
    }
    // The source, resolved once. An event rule naming nothing is refused: it reads no event, and under the
    // previous spelling `when_event: []` silently made the rule an ordinary span rule instead - a different
    // entry point from the one it was written for.
    let compiled_source = match source {
        None => CompiledSource::Span(super::schema::MessageStage::default()),
        Some(super::schema::MessageSource::Span(span)) => CompiledSource::Span(span.stage),
        Some(super::schema::MessageSource::Event(event)) => {
            if event.names.is_empty() {
                return Err(inexpressible(
                    "an event source naming no event reads nothing - remove the `source` to read a span's \
                     attributes, or name the events",
                ));
            }
            let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            if event.names.iter().any(|name| name.is_empty()) {
                return Err(inexpressible("an event source names an empty event"));
            }
            if event.names.iter().any(|name| !seen.insert(name.as_str())) {
                return Err(inexpressible(
                    "an event source names one event twice, which would read it twice",
                ));
            }
            CompiledSource::Event(event.names.clone())
        }
    };

    let compiled_branch_set = match branch_set {
        Some(set) => {
            let compile_group =
                    |group: &Vec<MessageRule>| -> Result<Vec<CompiledMessageRule>, MessageCompileError> {
                        group
                                .iter()
                                .map(|sub| {
                                    // The mirror of the parent's no-dead-fields rule. A leaf is reached
                                    // through its parent, so the fields the *entry points* consult are read
                                    // from the parent alone: `source` selects which entry point runs the
                                    // rule at all, and a branch's order is positional, so a leaf's rank
                                    // orders nothing. Each compiled silently and stated something the
                                    // engine never reads.
                                    if sub.source.is_some() || sub.legacy_rank.is_some() {
                                        return Err(inexpressible(
                                            "a branch leaf is reached through its parent, so `source` and \
                                             `legacy_rank` are read from the parent and would be ignored \
                                             here - declare them on the rule that owns the branch set",
                                        ));
                                    }
                                    compile_rule(file_id, sub, fragments)
                                })
                                .collect()
                    };
            if set
                .primary
                .iter()
                .chain(&set.fallback_if_primary_empty)
                .chain(&set.always)
                .any(|sub| sub.branch_set.is_some())
            {
                return Err(inexpressible(
                    "a branch set inside a branch set would be a control structure rather than a \
                         declaration",
                ));
            }
            if set.primary.is_empty() {
                return Err(inexpressible("a branch set declares no `primary` reading"));
            }
            // A branch parent is a *seam*: `emit_rule` delegates to its leaves immediately, so anything it
            // declares about reading or emitting is dead. Only its id, rank, documentation, gates and the
            // branch configuration are read - and a dead declaration is worse than a refused one, because it
            // reads as a statement the engine never makes.
            if read.named_count() != 0
                || compose.is_some()
                || wrap.is_some()
                || sections.is_some()
                || elements.is_some()
                || walk.is_some()
                || tool_repr.is_some()
                || aggregate_into_array.is_some()
                || !alternatives.is_empty()
                || !also.is_empty()
                || !fallback.is_empty()
                || emit.is_some()
                // Each of these was accepted and ignored. `reads_tool_spans` is the observable one: the
                // permission is read from the *leaves* (`reads_tool_spans_anywhere`), so a parent granting it
                // over ordinary leaves made the whole branch silently skipped on a tool span. `parse`,
                // `tag_as` and the two emptiness requirements describe a reading the parent does not perform.
                || parse.is_some()
                || tag_as.is_some()
                || require_non_empty.is_some()
                || require_non_blank.is_some()
                || reads_tool_spans.is_some()
                || require_members.is_some()
            {
                return Err(inexpressible(
                    "a branch set delegates to its leaves, so a carrier, an envelope, a reading, a \
                         requirement, a tool-span permission or a target on the parent would be ignored - \
                         declare it on the leaf that means it",
                ));
            }
            Some(CompiledBranchSet {
                primary: compile_group(&set.primary)?,
                fallback: compile_group(&set.fallback_if_primary_empty)?,
                always: compile_group(&set.always)?,
            })
        }
        None => None,
    };
    Ok(CompiledMessageRule {
        tool_repr: tool_repr.clone(),
        rule_file: file_id.to_string(),
        rule_id: id.clone(),
        doc: doc.clone(),
        read: read.clone(),
        compose: compose.as_ref().map(compile_compose),
        parse: *parse,
        require_members: require_members.clone(),
        wrap: wrap.clone(),
        target: emit_target,
        aggregate_into_array: aggregate,
        when: when.as_ref().map(super::detect_rules::compile_signals),
        unless: unless.as_ref().map(super::detect_rules::compile_signals),
        instrumentation_scope: instrumentation_scope.clone(),
        require_non_empty: non_empty,
        require_non_blank: non_blank,
        branch_set: compiled_branch_set,
        source: compiled_source,
        elements: elements.clone(),
        walk: walk.clone(),
        sections: sections.clone(),
        reads_tool_spans: tool_spans,
        tag_as: tag_as.clone(),
        alternatives: inline_fragments(alternatives, fragments)?,
        also: inline_fragments(also, fragments)?,
        fallback: inline_fragments(fallback, fragments)?,
        legacy_rank: legacy_rank.unwrap_or(0),
    })
}
