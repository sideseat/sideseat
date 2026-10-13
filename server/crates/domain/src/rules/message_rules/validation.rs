use super::*;

mod predicate_defects;

pub(super) use predicate_defects::raw_where_defect;
pub(in crate::rules) use predicate_defects::{member_predicate_defect, predicate_defect};

/// Every predicate set a compiled rule holds, wherever the declaration put it.
pub(super) fn predicate_sets(rule: &CompiledMessageRule) -> Vec<&ValueCondition> {
    let mut out = vec![&rule.raw_where];
    out.extend(rule.stream.iter().flat_map(CompiledStream::conditions));
    if let Some(set) = &rule.branch_set {
        for sub in set.primary.iter().chain(&set.fallback).chain(&set.always) {
            out.extend(predicate_sets(sub));
        }
    }
    if let Some(compose) = &rule.compose {
        out.push(&compose.require);
    }
    if let Some(sections) = &rule.sections {
        out.extend(sections.routes.iter().map(|route| &route.skip_when));
    }
    // An array read pass by pass has predicates at two levels - the pass's own, and each derived case of its
    // grouping - and both are evaluated. This is a live path, not a latent one.
    if let Some(elements) = &rule.elements {
        for pass in &elements.passes {
            out.push(&pass.when);
            if let Some(group) = &pass.group {
                out.extend(group.by.iter().map(|case| &case.when));
            }
        }
    }
    if let Some(overlay) = &rule.read.overlay {
        out.push(&overlay.witness);
        out.push(&overlay.require);
    }
    let decodes = [
        rule.decode.as_ref(),
        rule.read.decode.as_ref(),
        rule.read
            .overlay
            .as_ref()
            .and_then(|overlay| overlay.decode.as_ref()),
    ];
    out.extend(decodes.into_iter().flatten().map(|decode| &decode.require));
    out.extend(rule.read.rendering.as_ref());
    for reading in rule
        .alternatives
        .iter()
        .chain(&rule.also)
        .chain(&rule.fallback)
    {
        // The reading's own sets, and every case a fragment or this selection point contributed - which the
        // pre-inlining pass could not reach.
        for spec in std::iter::once(&reading.spec).chain(reading.fragment_cases.iter()) {
            out.push(&spec.require);
            out.push(&spec.require_parent);
            out.extend(spec.rendering.as_ref());
            if let Some(wrap) = &spec.wrap {
                out.extend(wrap_predicate_sets(wrap));
            }
        }
    }
    if let Some(wrap) = &rule.wrap {
        out.extend(wrap_predicate_sets(wrap));
    }
    out
}

/// Every envelope a compiled rule holds, wherever the declaration put it - the same places
/// [`predicate_sets`] walks.
pub(super) fn wraps(rule: &CompiledMessageRule) -> Vec<&WrapSpec> {
    let mut out = Vec::new();
    if let Some(set) = &rule.branch_set {
        for sub in set.primary.iter().chain(&set.fallback).chain(&set.always) {
            out.extend(wraps(sub));
        }
    }
    for reading in rule
        .alternatives
        .iter()
        .chain(&rule.also)
        .chain(&rule.fallback)
    {
        for spec in std::iter::once(&reading.spec).chain(reading.fragment_cases.iter()) {
            out.extend(spec.wrap.as_ref());
        }
    }
    out.extend(rule.wrap.as_ref());
    out
}

/// Why an attachment can never read what it declares.
pub(super) fn attach_defect(attach: &AttachSpec) -> Option<&'static str> {
    if let Some(defect) = attach.pipe_defect() {
        return Some(defect);
    }
    // A source-less attachment is a literal member of the shape: nothing is read, so a `default` would
    // never be attached and a `parse` would decode nothing. Each reads as though it did something.
    if attach.from.is_none()
        && attach.from_value_any_of.is_empty()
        && attach.from_path.is_none()
        && attach.or_span_name_after().is_none()
    {
        if attach.default.is_some() {
            return Some(
                "names no source, so nothing is ever *missing* and the `default` would never be \
                 attached - a literal member states its `value` instead",
            );
        }
        if attach.parse.is_some() {
            return Some("names no source, so there is nothing for `parse` to decode");
        }
    }
    attach.select.as_ref()?;
    if attach.from.is_none() {
        return Some("`select` reads inside the `from` attribute, and the attachment names none");
    }
    if !matches!(
        attach.parse,
        Some(ParseMode::Json | ParseMode::JsonOrString | ParseMode::StringifiedArray)
    ) {
        return Some(
            "`select` reads a member of a structure, so the `from` attribute must be parsed as JSON - \
             as text it has no members and the attachment could never resolve",
        );
    }
    None
}

/// Every attachment an envelope holds, on the envelope itself and on the blocks it builds.
pub(super) fn wrap_attachments(wrap: &WrapSpec) -> impl Iterator<Item = &AttachSpec> {
    wrap.attach.iter().chain(
        wrap.block
            .iter()
            .chain(wrap.prepend_block.as_ref().map(|p| &p.block))
            .flat_map(|block| block.attach.iter()),
    )
}

/// Every predicate set an envelope holds.
pub(super) fn wrap_predicate_sets(wrap: &WrapSpec) -> Vec<&ValueCondition> {
    let mut out: Vec<&ValueCondition> = wrap.attach.iter().map(|attach| &attach.require).collect();
    if let Some(block) = &wrap.prepend_block {
        out.push(&block.require);
    }
    for block in wrap
        .block
        .iter()
        .chain(wrap.prepend_block.as_ref().map(|p| &p.block))
    {
        out.extend(block.attach.iter().map(|attach| &attach.require));
    }
    out
}
/// Whether a rule's claim on a carrier is *conditional* - on the span, or on nothing else having
/// supplied the value.
///
/// This is what separates a genuine conflict from a shared carrier. The check exists to catch a rule that
/// can never emit, and a rule whose claim is conditional is not that: it fires on the spans its condition
/// admits and yields to the other elsewhere, with `priority` deciding which is tried first. Two
/// *unconditional* rules on one carrier really are a defect - the second could never run.
///
/// Reachable in practice: the generic `output.value` is read by one dialect as a gated last resort and by
/// another as its own output, and they are told apart by the span they are on.
pub(super) fn rule_condition(rule: &CompiledMessageRule) -> Condition {
    // Both facets, not one label. A rule can be gated *and* payload-narrowed, and folding them into one
    // value made "same gate, mutually exclusive payloads" look like a dead pair while it is a working one.
    // The gate's comparable part: its positive conjuncts. A negated conjunct narrows in the opposite direction -
    // the rule runs where something does *not* hold, and nothing here can relate that to another rule's positive
    // gate - and a scope conjunct is a telemetry-envelope gate that leaves the carrier free outside that exact
    // scope; treating either as unconditional would make a producer-specific decoder appear to permanently
    // suppress the convention's general reading of the same attribute. Both make the claim incomparable.
    let (gate, opaque) = match &rule.gate {
        None => (None, false),
        Some(gate) => {
            let conjuncts: Vec<&SpanExpr> = match gate {
                Expr::All(group) => group.children().iter().collect(),
                other => vec![other],
            };
            let comparable: Vec<SpanExpr> = conjuncts
                .iter()
                .filter(|conjunct| !span_conditions::is_opaque(conjunct))
                .map(|conjunct| (*conjunct).clone())
                .collect();
            let opaque = comparable.len() < conjuncts.len();
            (Expr::all(comparable), opaque)
        }
    };
    let parsed_as = text_parse(rule);
    if opaque {
        return Condition {
            gate,
            narrowed: true,
            parsed_as,
        };
    }
    // Every reading this rule can produce is gated on the payload itself, so it claims nothing on a span
    // whose payload no reading recognises. Asked of **all three** lists, because `all_readings` emits
    // through `also` and through `fallback` as well - checking `alternatives` alone let a rule with one
    // required alternative and an unconditional fallback claim its carrier on every span while counting as
    // conditional, which is a permanently dead second rule.
    let readings = rule
        .alternatives
        .iter()
        .chain(&rule.also)
        .chain(&rule.fallback);
    let mut any = false;
    let mut every_reading_narrowed = true;
    for reading in readings {
        any = true;
        // `require_parent` narrows a reading exactly as `require` does - it was absent here, so a rule
        // conditional only through it counted as unconditional and could falsely convict a valid fallback.
        if reading.spec.require.is_empty() && reading.spec.require_parent.is_empty() {
            every_reading_narrowed = false;
        }
    }
    // `raw_where` is asked of the carrier's raw text before anything is read, so a rule declaring one reads
    // nothing on a span whose text it refuses - which is a claim narrowed by the payload, like a reading's. So is
    // a rule's own `decode`: a value whose member does not decode reads nothing, and a later reader of the same
    // carrier is reached. An entry decode narrows nothing, since an undecoded entry is still emitted.
    Condition {
        gate,
        narrowed: (any && every_reading_narrowed)
            || !rule.raw_where.is_empty()
            || rule.decode.is_some(),
        parsed_as,
    }
}

/// A JSONPath rendered so two spellings of one path compare equal.
///
/// `$.v` and `$['v']` select the same member and render differently, so a same-path check on the rendered
/// string missed a contradiction written the second way. Only the identifier case is folded, because that is
/// the whole of the ambiguity: a bracket segment holding anything else has no dot spelling.
pub(super) fn canonical_path(path: &super::schema::JsonPath) -> String {
    let rendered = path.to_string();
    // A filter's string literal can contain anything, including `['v']` and `.v`, and a textual fold cannot
    // see that it is inside one - it equated `$[?@.x == "['v']"]` with `$[?@.x == ".v"]`, which are different
    // conditions. So a path holding a filter, or any escape, is left exactly as rendered: two spellings then
    // compare unequal and the tautology check does not fire, which under-refuses rather than convicting a
    // real condition. Every fold given up here is on a shape no asset writes.
    // A double-quoted name can contain `['v']` verbatim - `$["['v']"]` is a member spelled with brackets -
    // and the same textual blindness applies, so any double quote gives the fold up too.
    if rendered.contains('?') || rendered.contains('\\') || rendered.contains('"') {
        return rendered;
    }
    let mut out = String::with_capacity(rendered.len());
    let mut rest = rendered.as_str();
    while let Some(open) = rest.find("['") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("']") else { break };
        let name = &after[..close];
        // `is_alphanumeric`, not the ASCII form: `$.é` and `$['é']` select the same member, and an
        // ASCII-only fold left them different.
        let foldable = !name.is_empty()
            && name.chars().all(|c| c.is_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_numeric());
        out.push_str(&rest[..open]);
        if foldable {
            out.push('.');
            out.push_str(name);
        } else {
            out.push_str(&rest[open..open + 2 + close + 2]);
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

/// The one carrier every emission of this rule owns, where the rule leaves no choice about it.
///
/// Deliberately narrow: a single named attribute, and nothing else that could contribute another carrier or
/// choose between several. Anything looser is not a *proof* that two emissions are resolved by ownership, and
/// an unproven exemption is how two messages end up under one carrier tag with nothing to tell them apart.
pub(super) fn necessarily_owned(rule: &CompiledMessageRule) -> Option<&str> {
    if rule.branch_set.is_some() || rule.read.overlay.is_some() {
        return None;
    }
    // **A compose owns its members' carriers and not its own tag.** The exemption here said the opposite -
    // that two composes sharing a tag are resolved by ownership whatever their sources are - and the runtime
    // agreed by pushing the synthetic tag into `owns`. So two composes assembling the same canonical shape from
    // *different* physical carriers suppressed each other: the earlier one claimed a name no producer wrote,
    // and the later one's members went unread. Ownership is over what a span carried; a synthetic tag is what
    // the engine calls the result.
    if rule.compose.is_some() {
        return None;
    }
    if rule.read.indexed_family.is_some() {
        return None;
    }
    // `each` names several carriers that are all read, so there is no single one to answer with - unlike
    // `first_present`, where exactly one spelling is read per span.
    if !rule.read.each.is_empty() {
        return None;
    }
    match rule.read.first_present() {
        // One spelling is not a choice.
        [only] if rule.read.attribute().is_none() => Some(only.as_str()),
        [] => rule.read.attribute().map(String::as_str),
        _ => None,
    }
}

/// What narrows a claim on a carrier: the span it runs on, and whether the payload narrows it further.
///
/// Three independent facets, because a rule can have all of them and they answer different questions. The gate
/// says *which spans* the rule runs on and can be related to another rule's gate; `narrowed` says the rule may
/// read nothing even where it runs, for a reason nothing here can compare with another rule's; `parsed_as` says
/// which texts of the carrier it can read at all, which *can* be compared with another rule's.
#[derive(Clone, Default)]
pub(super) struct Condition {
    /// The spans this runs on. `None` means every span.
    pub(super) gate: Option<SpanExpr>,
    /// Narrowed by the payload, by a parent, or by which spelling of the carrier a producer used.
    pub(super) narrowed: bool,
    /// The parse the carrier's text must pass before anything is read from it; `None` where the reading does not
    /// parse the text. A text the parse refuses yields nothing and claims nothing, so it is left to a later rule
    /// that reads it another way.
    pub(super) parsed_as: Option<ParseMode>,
}

/// The parse a rule applies to its carrier's whole text, where one decides whether the rule reads it at all.
///
/// Only the scalar readings parse the text: a section split reads it as text, and a compose, a branch or a family
/// assemble their value from members with parses of their own.
fn text_parse(rule: &CompiledMessageRule) -> Option<ParseMode> {
    let assembled = rule.compose.is_some()
        || rule.branch_set.is_some()
        || rule.read.indexed_family.is_some()
        || rule.read.family.is_some()
        || rule.sections.is_some();
    (!assembled).then(|| rule.parse.unwrap_or(ParseMode::Json))
}

/// Whether every text a reading under `theirs` accepts is one a reading under `mine` accepts too - sound and
/// deliberately incomplete: where it cannot tell, it answers no, so a later rule is not convicted.
fn reads_whatever(mine: Option<ParseMode>, theirs: Option<ParseMode>) -> bool {
    let every_text = |mode: ParseMode| {
        matches!(
            mode,
            ParseMode::Text | ParseMode::JsonOrString | ParseMode::JsonStructureOrString
        )
    };
    match (mine, theirs) {
        (None, _) => true,
        (Some(mine), _) if every_text(mine) => true,
        (Some(mine), Some(theirs)) if mine == theirs => true,
        // Both decode the text as JSON first and refuse what does not decode, so JSON covers them.
        (
            Some(ParseMode::Json),
            Some(ParseMode::StringifiedArray | ParseMode::PythonConstructorReprArray),
        ) => true,
        _ => false,
    }
}

impl Condition {
    /// Whether a claim under `self` **suppresses** one under `other`: it runs wherever the other does, and
    /// where it runs it always claims.
    ///
    /// Directional and rank-aware - the caller passes the earlier rule as `self` - because that is the actual
    /// question: is the later rule dead? Equality was the wrong relation. Gates are *disjunctions* of
    /// signals, so `attr_exists: ["a", "b"]` holds everywhere `attr_exists: ["a"]` does and suppresses it,
    /// while their declared forms differ.
    ///
    /// `narrowed` on the earlier side is what makes the pair safe: it may read nothing on a span it runs on,
    /// leaving the carrier for the later rule. Two *identical* payload requirements are therefore accepted -
    /// comparing payload predicates would be a satisfiability decision this deliberately is not, and
    /// `no_declared_rule_is_dead_across_the_corpus` is what measures that case instead.
    pub(super) fn suppresses(&self, other: &Self) -> bool {
        if self.narrowed {
            return false;
        }
        // A text this rule's parse refuses is one it does not claim, so a later rule reading that text another way
        // is live there: a JSON reading beside a text reading of one attribute is a decoder and its fallback.
        if !reads_whatever(self.parsed_as, other.parsed_as) {
            return false;
        }
        match (&self.gate, &other.gate) {
            // Ungated: runs on every span, so it runs wherever anything else does.
            (None, _) => true,
            // Gated against ungated: the other runs on spans this one does not.
            (Some(_), None) => false,
            (Some(mine), Some(theirs)) => gate_covers(mine, theirs),
        }
    }
}

/// Whether every span `narrower` admits is also admitted by `wider`: the conditions' implication, which is sound
/// and deliberately incomplete. A missed implication under-refuses, which leaves a dead rule to the corpus
/// measurement, while a wrong one deletes a working rule at startup.
pub(super) fn gate_covers(wider: &SpanExpr, narrower: &SpanExpr) -> bool {
    span_conditions::implies(narrower, wider)
}

/// One carrier a rule reads, and whether *that* claim is conditional.
///
/// Per pattern rather than per rule, because a rule can hold both kinds at once: an indexed family it always
/// reads, beside a side payload it reads only where a witness holds.
#[derive(Clone)]
pub(super) struct Consumed {
    pub(super) pattern: CarrierPattern,
    pub(super) condition: Condition,
}

/// What a rule reads, each carrier paired with what narrows the claim on it.
pub(super) fn consumed_carriers(rule: &CompiledMessageRule) -> Vec<Consumed> {
    narrow_with(consumed_patterns(rule), &rule_condition(rule))
}

/// Apply a rule-wide condition to patterns that do not already carry a narrower one of their own.
///
/// A **branch leaf keeps its own gate**. Flattening replaced every child condition with the parent's, so an
/// ungated parent made a `when`-gated leaf look unconditional and the leaf could then convict a rule that is
/// live on spans its gate excludes. A leaf's gate is at least as narrow as its parent's - the parent's gate is
/// checked first and then the leaf's - so combining them means keeping the leaf's where it has one.
pub(super) fn narrow_with(patterns: Vec<Consumed>, wholly: &Condition) -> Vec<Consumed> {
    patterns
        .into_iter()
        .map(|mut consumed| {
            // Every pattern here is the rule's own, so the parse it applies to the text is the rule's.
            consumed.condition.parsed_as = wholly.parsed_as;
            if consumed.condition.narrowed {
                // Already the narrowest answer available: this carrier is read only sometimes, for a reason
                // nothing here can relate to another rule's. A rule-wide condition cannot widen that.
                return consumed;
            }
            match (&consumed.condition.gate, &wholly.gate) {
                // Nothing of its own: the rule-wide condition is the whole answer.
                (None, _) => consumed.condition = wholly.clone(),
                // A gate of its own and none above it: keep it, and inherit any payload narrowing.
                (Some(_), None) => consumed.condition.narrowed |= wholly.narrowed,
                // **Both**, which runtime evaluates as a conjunction - the parent's gate is checked and then
                // the leaf's. Keeping the leaf's alone claimed the rule runs wherever that gate holds, which
                // is false where the parent's does not, and it convicted a rule live on exactly those spans.
                // Where one gate provably covers the other the conjunction *is* the narrower of the two;
                // otherwise nothing here can express it, so the claim is opaque.
                (Some(leaf), Some(parent)) => {
                    if gate_covers(parent, leaf) {
                        // The parent admits every span the leaf does, so the leaf's gate is the conjunction.
                        consumed.condition.narrowed |= wholly.narrowed;
                    } else if gate_covers(leaf, parent) {
                        consumed.condition = wholly.clone();
                    } else {
                        consumed.condition.narrowed = true;
                    }
                }
            }
            consumed
        })
        .collect()
}

pub(super) fn consumed_patterns(rule: &CompiledMessageRule) -> Vec<Consumed> {
    let mut out: Vec<Consumed> = Vec::new();
    let always = |pattern: CarrierPattern| Consumed {
        pattern,
        condition: Condition::default(),
    };
    let only_sometimes = |pattern: CarrierPattern| Consumed {
        pattern,
        condition: Condition {
            gate: None,
            narrowed: true,
            parsed_as: None,
        },
    };
    if let Some(attribute) = rule.read.attribute().map(String::as_str) {
        out.push(always(CarrierPattern::Exact(attribute.to_string())));
    }
    // Only the *first* alternative is claimed unconditionally: the rest are read where no earlier spelling
    // was present, so a rule reading a later one yields whenever an earlier one is there.
    for (position, key) in rule.read.first_present().iter().enumerate() {
        let pattern = CarrierPattern::Exact(key.clone());
        out.push(if position == 0 {
            always(pattern)
        } else {
            only_sometimes(pattern)
        });
    }
    // `each` is the opposite: every listed key the span carries is read, so every one is claimed
    // unconditionally. Under the conflated member this analysis said `only_sometimes` for all but the first,
    // which understated what the one shipped `each` rule owns.
    for key in &rule.read.each {
        out.push(always(CarrierPattern::Exact(key.clone())));
    }
    if let Some(family) = rule.read.indexed_family.as_deref() {
        // Every key beneath the family, since each index's members are read.
        //
        // Conditional where members are required: an entry lacking them contributes nothing, so a second rule
        // reading one of the family's keys is live on a span whose entries this rule rejects.
        let pattern = CarrierPattern::Prefix(format!("{family}."));
        out.push(if rule.require_members.is_some() {
            only_sometimes(pattern)
        } else {
            always(pattern)
        });
    }
    if let Some(family) = rule.read.family.as_deref() {
        out.push(always(CarrierPattern::Prefix(family.to_string())));
    }
    if let Some(overlay) = &rule.read.overlay {
        // The payload a positional overlay joins against is read too, and it is not beneath the family.
        //
        // Conditional where the join is witnessed: the rule consumes this payload only on spans whose
        // counterpart list is this dialect's own serialisation, and yields it elsewhere. The *family* above
        // stays unconditional, which is the distinction a rule-wide flag could not express.
        // `require` narrows the same way a witness does: the joined content has to satisfy it, and where it
        // does not the payload is not consumed.
        let pattern = CarrierPattern::Exact(overlay.from.clone());
        out.push(
            if overlay.witness.is_empty() && overlay.require.is_empty() {
                always(pattern)
            } else {
                only_sometimes(pattern)
            },
        );
    }
    // A branch set's subrules read carriers of their own, and they were invisible here - so two dialects
    // could contend for one carrier as long as the collision was inside a branch set.
    if let Some(set) = &rule.branch_set {
        for sub in set.primary.iter().chain(&set.always) {
            out.extend(consumed_carriers(sub));
        }
        // A `fallback_if_primary_empty` leaf reads only where every primary reading came up empty - a
        // condition on the payload that nothing here can relate to another rule's, so it is narrowed.
        for sub in &set.fallback {
            out.extend(consumed_carriers(sub).into_iter().map(|mut c| {
                c.condition.narrowed = true;
                c
            }));
        }
    }
    if let Some(compose) = &rule.compose {
        for member in compose.members.iter().map(|m| &m.spec) {
            // The first spelling is the one this member always takes; a later one is read only where the
            // earlier is absent.
            for (position, key) in member.from_any_of.iter().enumerate() {
                let pattern = CarrierPattern::Exact(key.clone());
                out.push(if position == 0 {
                    always(pattern)
                } else {
                    only_sometimes(pattern)
                });
            }
            if let Some(fallback) = &member.fallback {
                // Read only where the member's own gate holds, which is what makes a dialect's stand-in for
                // the generic pair coexist with the dialect that owns it.
                out.push(only_sometimes(CarrierPattern::Exact(fallback.from.clone())));
            }
            if let Some(prefix) = &member.sweep_prefix {
                out.push(always(CarrierPattern::Prefix(prefix.clone())));
            }
        }
    }
    out
}

/// What a rule tags its observations with.
pub(super) fn emitted_carriers(rule: &CompiledMessageRule) -> Vec<Consumed> {
    narrow_with(emitted_patterns(rule), &rule_condition(rule))
}

/// The tags themselves, each with whatever narrows *that* emission.
pub(super) fn emitted_patterns(rule: &CompiledMessageRule) -> Vec<Consumed> {
    let always = |pattern: CarrierPattern| Consumed {
        pattern,
        condition: Condition::default(),
    };
    // A branch set emits what its sub-rules emit - each is a rule in its own right, and one with `tag_as`
    // emits a carrier this rule never names. Invisible here, two dialects could both emit one carrier from
    // inside their branch sets.
    if let Some(set) = &rule.branch_set {
        let mut out: Vec<Consumed> = set
            .primary
            .iter()
            .chain(&set.always)
            .flat_map(emitted_carriers)
            .collect();
        out.extend(set.fallback.iter().flat_map(emitted_carriers).map(|mut c| {
            c.condition.narrowed = true;
            c
        }));
        return out;
    }
    if let Some(tag) = &rule.tag_as {
        // Overrides every read form: whatever was read, this is the tag.
        return vec![always(CarrierPattern::Exact(tag.clone()))];
    }
    if let Some(compose) = &rule.compose {
        return vec![always(CarrierPattern::Exact(compose.tag.clone()))];
    }
    let mut out = Vec::new();
    if let Some(attribute) = rule.read.attribute().map(String::as_str) {
        out.push(always(CarrierPattern::Exact(attribute.to_string())));
    }
    // A tag per spelling, and only the first is emitted whatever the span carries.
    for key in &rule.read.each {
        out.push(always(CarrierPattern::Exact(key.clone())));
    }
    for (position, key) in rule.read.first_present().iter().enumerate() {
        let pattern = CarrierPattern::Exact(key.clone());
        out.push(if position == 0 {
            always(pattern)
        } else {
            Consumed {
                pattern,
                condition: Condition {
                    gate: None,
                    narrowed: true,
                    parsed_as: None,
                },
            }
        });
    }
    if let Some(family) = rule.read.indexed_family.as_deref() {
        // One tag per index, and per sub-level where there is one - a prefix covers them all. Emitted only
        // for entries that satisfy the required members, which is why the condition mirrors the read side.
        let pattern = CarrierPattern::Prefix(format!("{family}."));
        out.push(if rule.require_members.is_some() {
            Consumed {
                pattern,
                condition: Condition {
                    gate: None,
                    narrowed: true,
                    parsed_as: None,
                },
            }
        } else {
            always(pattern)
        });
    }
    out
}

/// Why a rule's `rendering` declarations cannot mean what they say.
///
/// A rendering is a *message* the trace and session views leave out, so marking anything else - a tool
/// definition, a name list, a claim - states a view rule for something no view shows. An aggregate emits one
/// observation for every entry, so one entry being a rendering has no observation to mark. And on a read, the
/// marker is asked of an indexed family's entries; any other read form has its readings, which carry their own.
pub(super) fn rendering_defect(rule: &CompiledMessageRule) -> Option<&'static str> {
    if let Some(set) = &rule.branch_set
        && let Some(defect) = set
            .primary
            .iter()
            .chain(&set.fallback)
            .chain(&set.always)
            .find_map(rendering_defect)
    {
        return Some(defect);
    }
    let readings: Vec<&Alternative> = rule
        .alternatives
        .iter()
        .chain(&rule.also)
        .chain(&rule.fallback)
        .flat_map(|reading| std::iter::once(&reading.spec).chain(reading.fragment_cases.iter()))
        .collect();
    let marked_readings: Vec<&&Alternative> = readings
        .iter()
        .filter(|spec| spec.rendering.is_some())
        .collect();
    if rule.read.rendering.is_some() && rule.read.indexed_family.is_none() {
        return Some(
            "declares `rendering` on a read that is not an indexed family - a reading's own `rendering` is \
             where the marker goes",
        );
    }
    if rule.aggregate_into_array && (rule.read.rendering.is_some() || !marked_readings.is_empty()) {
        return Some(
            "marks renderings in an aggregate, which emits one observation for all of its entries, so no \
             observation is the rendering",
        );
    }
    let emits_messages = |target: EmitTarget| target == EmitTarget::Message;
    if (rule.read.rendering.is_some() && !emits_messages(rule.target))
        || marked_readings
            .iter()
            .any(|spec| !emits_messages(spec.emit.unwrap_or(rule.target)))
    {
        return Some(
            "marks as a rendering a reading that does not emit messages - only a message is left out of a view",
        );
    }
    None
}

/// Why a walk's declaration can never mean what it says: a `stop_on`, a `taken_by` or a `prune_read_by` naming
/// a clause the rule does not declare, or a `prune_read_by` clause that reads its members only in part.
pub(super) fn walk_defect(
    walk: &super::schema::WalkSpec,
    alternatives: &[Alternative],
    also: &[Alternative],
    fallback: &[Alternative],
    fragments: &HashMap<String, Vec<Alternative>>,
) -> Option<&'static str> {
    // **A walk's `stop_on` names clauses of its own rule.** A name that matches nothing can never stop the
    // descent, so the walk silently runs to `max_depth` - and an id that is merely misspelled looks exactly like
    // a deliberate "never stop".
    let local: std::collections::BTreeSet<&str> = alternatives
        .iter()
        .chain(also.iter())
        .chain(fallback.iter())
        .map(|reading| reading.id.as_str())
        .collect();
    if walk.stop_on.iter().any(|id| !local.contains(id.as_str())) {
        return Some(
            "a walk's `stop_on` names a clause this rule does not declare, so it could never stop the \
                     descent - which is indistinguishable from meaning never to stop",
        );
    }
    // The same for a pruned member's `taken_by`: a clause the rule does not declare never recognises
    // anything, so the member is never pruned - a misspelling that reads as a deliberate prune.
    if walk
        .prune
        .iter()
        .map(|pruned| pruned.taken_by.as_str())
        .chain(walk.prune_read_by.iter().map(String::as_str))
        .any(|id| !local.contains(id))
    {
        return Some(
            "a walk prunes a member `taken_by` a clause this rule does not declare, so nothing could \
                     ever take it and the prune never happens",
        );
    }
    // A `prune_read_by` clause must read the member it is recorded against **whole**: one that descends,
    // picks a sub-member, or collects some members reads part of it, and pruning the member would hide
    // the parts it did not read. Asked of the clause and of every fragment case it hands its elements to.
    let partial = |reading: &Alternative| {
        reading.descend.is_some()
            || !reading.then_present_any_of.is_empty()
            || reading.collect_members.is_some()
    };
    // A case the clause hands each element to reads that element - so it may not select inside it or iterate
    // it, which would read some of its members and let the element be pruned with the rest unread. What a
    // case then builds is built from the element itself, which declares the element *the* message: its other
    // members are that message's, exactly as a `stop_on` clause recognising a node means.
    let partial_case = |case: &Alternative| {
        partial(case)
            || case.each
            || case
                .select
                .as_ref()
                .is_some_and(|path| path.to_string() != "$")
    };
    let partially_read = alternatives
        .iter()
        .chain(also.iter())
        .chain(fallback.iter())
        .filter(|reading| walk.prune_read_by.contains(&reading.id))
        .any(|reading| {
            partial(reading)
                || reading.extra_cases.iter().any(partial_case)
                || reading.then_fragment.as_deref().is_some_and(|name| {
                    fragments
                        .get(name)
                        .is_some_and(|cases| cases.iter().any(partial_case))
                })
        });
    if partially_read {
        return Some(
            "a walk prunes the members a clause read, and that clause reads only part of each - it \
                     descends, picks a sub-member or collects some members - so pruning would hide the parts \
                     it did not read",
        );
    }
    None
}
