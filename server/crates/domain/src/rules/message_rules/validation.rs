use super::*;

/// A **selected** set of predicate defects, refused like any other no-op.
///
/// Not a satisfiability decision procedure, and the distinction is load-bearing: it proves chosen defects at
/// the *root*, and accepts everything else - including a genuine contradiction on a singular member path.
/// Under-refusing is the right failure, because an over-refusal deletes a working rule, and every
/// over-refusal in this area has been one of mine.
///
/// One function, applied by one recursive pass over every predicate-bearing place a *compiled* rule has -
/// after fragments and extra cases are inlined. Checking the direct alternatives only left the same
/// contradiction reachable through `require_parent`, an attachment, an overlay, a prepended block, or any
/// case a fragment contributed: the pass that ran before inlining could not see those at all.
pub(in crate::rules) fn predicate_defect(condition: &ValueCondition) -> Option<&'static str> {
    // A condition of the two-list shape is analysed as one, cross-member contradictions included; any other shape
    // has its atoms checked one by one, with the polarity each sits at, and none of the cross-member checks.
    match condition.set_view() {
        Some(set) => set_defect(&set),
        None => condition
            .declared()
            .and_then(|expr| polar_atom_defect(expr, true)),
    }
}

/// The per-atom defects of an expression of any shape, each atom judged at its polarity.
///
/// Polarity matters because a contradiction is a defect only where it is asserted. Under a `not` it is a
/// condition: `not({path: "$.x", kind: "null", not_null: true})` holds whenever `x` exists and is unknown when
/// it is absent - an odd spelling, and still a statement some value satisfies and some does not.
fn polar_atom_defect(
    expr: &crate::rules::expr::Expr<ValuePredicate>,
    positive: bool,
) -> Option<&'static str> {
    use crate::rules::expr::Expr;
    match expr {
        Expr::Atom(predicate) => atom_defect(predicate, positive),
        Expr::All(group) | Expr::Any(group) => group
            .children()
            .iter()
            .find_map(|child| polar_atom_defect(child, positive)),
        Expr::Not(child) => polar_atom_defect(child, !positive),
    }
}

/// The defects of a two-list predicate set.
fn set_defect(set: &PredicateSet) -> Option<&'static str> {
    // Defects between *members* of a set, which no per-predicate check can see - and restricted to the
    // **root**, because only there is the reasoning sound. My first version compared any two members on the
    // same path and was wrong three ways at once:
    //
    // - On a *member* path, `not_null: true` beside `not_null: false` is not a tautology: when the member is
    //   absent both fail, so the pair means "the member exists".
    // - A *plural* path makes each predicate existential, so `kind: string` and `kind: number` can both hold
    //   against different matches of `$.*` - an ordinary statement the check refused.
    // - Two spellings of one path (`$.v`, `$['v']`) render differently, so an equivalent-path contradiction
    //   escaped while a satisfiable same-path pair was refused.
    //
    // At the root all three disappear: the value always exists, there is exactly one of it, and the path is
    // either absent or `$`. That is also where the defect that prompted this lives - a content-block rule
    // whose `require` holds for every block it is offered.
    let on_root = |p: &ValuePredicate| p.path.as_ref().is_none_or(|path| path.to_string() == "$");
    // How many conditions a predicate asserts. A complement pair is a tautology only when *neither* side
    // narrows any further: `{kind: string, not_null: true}` beside `{not_null: false}` is false for a
    // non-null number, so the extra `kind` makes the pair an ordinary statement.
    let sole = |p: &ValuePredicate| -> bool {
        usize::from(p.exists.is_some())
            + usize::from(p.kind.is_some())
            + usize::from(p.non_empty.is_some())
            + usize::from(p.non_blank.is_some())
            + usize::from(p.not_null.is_some())
            + usize::from(p.identifier_like.is_some())
            + usize::from(p.starts_with.is_some())
            + usize::from(p.lacks_prefix.is_some())
            + usize::from(!p.one_of.is_empty())
            + usize::from(!p.none_of.is_empty())
            + usize::from(p.equals.is_some())
            + usize::from(!p.only_members.is_empty())
            == 1
    };
    // A pair one side or the other of which holds of *every* root value. Only total tests qualify: `not_null`
    // answers for every value, and a prefix is had or lacked by every string while a non-string lacks it.
    let complements = |a: &ValuePredicate, b: &ValuePredicate| -> bool {
        sole(a)
            && sole(b)
            && ((a.not_null == Some(true) && b.not_null == Some(false))
                || (a.not_null == Some(false) && b.not_null == Some(true))
                || (a.starts_with.is_some() && a.starts_with == b.lacks_prefix)
                || (b.starts_with.is_some() && b.starts_with == a.lacks_prefix))
    };
    // A pair no root value satisfies both of: every complement, and the partial tests too. `non_empty` and
    // `identifier_like` are unknown outside the kinds they test, so their pair under `any` is a filter on those
    // kinds rather than a tautology - but under `all` neither value can be both, and a kind they cannot answer
    // leaves the pair unknown, which does not hold either.
    let contradicts = |a: &ValuePredicate, b: &ValuePredicate| -> bool {
        complements(a, b)
            || (sole(a)
                && sole(b)
                && ((a.non_empty.is_some() && b.non_empty.is_some() && a.non_empty != b.non_empty)
                    || (a.identifier_like.is_some()
                        && b.identifier_like.is_some()
                        && a.identifier_like != b.identifier_like)))
    };
    // `exists` is the one complement that is a tautology on **any** path, and it is exactly why: it is the
    // predicate that decides presence, so "present" beside "absent" covers every value there is. On a
    // singular path one branch or the other always holds; on a plural one a match either exists (the first
    // branch) or there are none and the singular branch answers `exists: false`. So this pair is checked
    // wherever it appears, unlike the value complements above, which a *member* path makes satisfiable.
    //
    // Load-bearing beyond being a no-op: `rule_is_wholly_conditional` reads "every reading carries a
    // `require`" as evidence that a rule yields its carrier on some span. A tautological `require` makes that
    // a false statement, and the second rule reading the same carrier is then permanently dead.
    let same_path = |a: &ValuePredicate, b: &ValuePredicate| match (&a.path, &b.path) {
        (Some(a), Some(b)) => canonical_path(a) == canonical_path(b),
        (None, None) => true,
        _ => false,
    };
    // A tautology needs every forbidden value to be required by the other branch: with
    // `none_of: ["a", "b"]` beside `one_of: ["a"]`, the value `"b"` satisfies neither.
    let covers = |required: &ValuePredicate, forbidden: &ValuePredicate| {
        sole(required)
            && sole(forbidden)
            && !forbidden.none_of.is_empty()
            && forbidden
                .none_of
                .iter()
                .all(|value| required.one_of.contains(value))
    };
    for (i, left) in set.any.iter().enumerate() {
        for right in &set.any[i + 1..] {
            if !same_path(left, right) {
                continue;
            }
            if sole(left)
                && sole(right)
                && ((left.exists == Some(true) && right.exists == Some(false))
                    || (left.exists == Some(false) && right.exists == Some(true)))
            {
                return Some(
                    "an `any` set requires a value to exist and to be absent, which holds of every value",
                );
            }
            if covers(left, right) || covers(right, left) {
                return Some(
                    "an `any` set forbids only values another member requires, and a sole `none_of` \
                     also holds of an absent value - so it holds for every value",
                );
            }
        }
    }
    let any_root: Vec<&ValuePredicate> = set.any.iter().filter(|p| on_root(p)).collect();
    let all_root: Vec<&ValuePredicate> = set.all.iter().filter(|p| on_root(p)).collect();
    for (i, left) in any_root.iter().enumerate() {
        for right in &any_root[i + 1..] {
            if complements(left, right) {
                return Some(
                    "an `any` set holds a root condition and its negation, so it holds for every value",
                );
            }
        }
    }
    let all_root_kinds: Vec<ValueKind> = all_root.iter().filter_map(|p| p.kind).collect();
    if let Some(first) = all_root_kinds.first()
        && all_root_kinds.iter().any(|kind| kind != first)
    {
        return Some("an `all` set names two kinds for the root, so it holds for nothing");
    }
    // The two checks below reason about *every* `any` member, so they hold only when every member is on the
    // root: a member path is a disjunct this analysis cannot see, and `kind: object` beside
    // `any(kind: string, $.x exists)` holds for `{"x": 1}`.
    let any_wholly_root = !set.any.is_empty() && any_root.len() == set.any.len();
    if let Some(required) = all_root_kinds.first()
        && any_wholly_root
        && any_root
            .iter()
            .all(|p| p.kind.is_some_and(|kind| kind != *required))
    {
        return Some(
            "an `all` set names one root kind and every `any` member names a different one, so it \
                 holds for nothing",
        );
    }
    // A kind that is not null cannot also be null. Reached across the branches, since the `all` side is
    // required and every `any` member must hold something compatible with it.
    if let Some(required) = all_root_kinds.first()
        && *required != ValueKind::Null
        && (all_root.iter().any(|p| p.not_null == Some(false))
            || (any_wholly_root && any_root.iter().all(|p| p.not_null == Some(false))))
    {
        return Some(
            "an `all` set requires a root kind that is not null while a required branch asserts the \
                 root is null, so it holds for nothing",
        );
    }
    for (i, left) in all_root.iter().enumerate() {
        for right in &all_root[i + 1..] {
            if contradicts(left, right) {
                return Some(
                    "an `all` set holds a root condition and its negation, so it holds for nothing",
                );
            }
        }
    }

    set.all
        .iter()
        .chain(set.any.iter())
        .find_map(|predicate| atom_defect(predicate, true))
}

/// The defects of one predicate, asserted (`positive`) or under a `not`.
///
/// Two classes. A predicate that *can never hold* is a defect where it is asserted, and a condition under a
/// negation. A predicate one of whose conditions is *ignored*, or that tests the root for nothing beyond
/// presence, is a defect at either polarity: an ignored condition is ignored under a `not` too, and the root's
/// presence is a tautology negated or not.
fn atom_defect(predicate: &ValuePredicate, positive: bool) -> Option<&'static str> {
    // A predicate on the **root** that asserts nothing beyond presence is a tautology: the value being
    // tested always exists. `{}`, `{"path": "$"}` and `{"exists": true}` are the same statement, and each
    // makes a rule that requires it recognise everything. A member *path* with no conditions is
    // different and stays legal - it asserts the member is there.
    let on_root = predicate
        .path
        .as_ref()
        .is_none_or(|path| path.to_string() == "$");
    if predicate.exists == Some(false)
        && (predicate.kind.is_some()
            || predicate.non_empty.is_some()
            || predicate.non_blank.is_some()
            || predicate.not_null.is_some()
            || predicate.identifier_like.is_some()
            || predicate.starts_with.is_some()
            || predicate.lacks_prefix.is_some()
            // `one_of` needs a value to be one of them, so it cannot hold on an absent member - and the
            // absent branch returns before consulting it, so it was silently ignored.
            || !predicate.one_of.is_empty()
            || predicate.equals.is_some()
            || !predicate.only_members.is_empty())
    {
        return Some(
            "`exists: false` asserts the member is absent, so no other condition on it \
             can hold",
        );
    }
    // `none_of` is the one condition an absent member satisfies - alone it holds for absence, which is how a
    // dialect's unnamed events fall through - so beside `exists: false` it states nothing, and the lowering
    // read the pair as a value both absent and present, which holds for nothing.
    if predicate.exists == Some(false) && !predicate.none_of.is_empty() {
        return Some(
            "`none_of` beside `exists: false` adds nothing - an absent member satisfies it - so write \
             `exists: false` alone",
        );
    }
    // The root always exists, so asserting its absence can never hold. Judged *before* the
    // presence-only rule below, which any other condition - `none_of`, say - would otherwise mask.
    if on_root && predicate.exists == Some(false) {
        return Some(
            "`exists: false` on the root can never hold - the value being tested is always there",
        );
    }
    let asserts_only_presence = predicate.kind.is_none()
        && predicate.non_empty.is_none()
        && predicate.non_blank.is_none()
        && predicate.not_null.is_none()
        && predicate.identifier_like.is_none()
        && predicate.starts_with.is_none()
        && predicate.lacks_prefix.is_none()
        && predicate.one_of.is_empty()
        && predicate.none_of.is_empty()
        && predicate.equals.is_none()
        && predicate.only_members.is_empty();
    if on_root && asserts_only_presence {
        return Some(
            "a predicate on the root that asserts nothing beyond presence is a tautology - the \
                 value being tested always exists, so the condition recognises everything",
        );
    }
    if !positive {
        return None;
    }
    if predicate.non_empty.is_some()
        && matches!(
            predicate.kind,
            Some(ValueKind::Number | ValueKind::Bool | ValueKind::Null)
        )
    {
        return Some(
            "`non_empty` is meaningless for a number, boolean or null - only strings, \
             arrays and objects can be empty",
        );
    }
    // Only when the required prefix *already begins with* the forbidden one: `starts_with: "ab"` with
    // `lacks_prefix: "a"` cannot hold. The reverse is satisfiable - `starts_with: "a"` beside
    // `lacks_prefix: "ab"` is met by `"ac"` - and refusing it refused a real rule.
    if let (Some(starts), Some(lacks)) = (&predicate.starts_with, &predicate.lacks_prefix)
        && starts.starts_with(lacks.as_str())
    {
        return Some(
            "`starts_with` begins with the prefix `lacks_prefix` forbids, so no value satisfies both",
        );
    }
    if matches!(predicate.kind, Some(ValueKind::Null)) && predicate.not_null == Some(true) {
        return Some("`kind: null` and `not_null: true` on one predicate");
    }
    if predicate.kind.is_some()
        && !matches!(predicate.kind, Some(ValueKind::Null))
        && predicate.not_null == Some(false)
    {
        return Some("a kind that is not null, beside `not_null: false`");
    }
    // Only when *every* value `one_of` allows is one `none_of` forbids. A partial overlap is satisfiable -
    // `{one_of: ["a", "b"], none_of: ["a"]}` is met by `"b"` - and refusing it refused a working predicate.
    if !predicate.one_of.is_empty()
        && predicate
            .one_of
            .iter()
            .all(|value| predicate.none_of.contains(value))
    {
        return Some(
            "every value `one_of` allows is one `none_of` forbids, so no value satisfies both",
        );
    }
    // Every text condition needs a string. Declared beside a kind that is not one, it can never hold - and a
    // predicate that can never hold is the same defect as one that asserts nothing. `lacks_prefix` is not one:
    // it holds of every value that is not a string, which is what the reference documents.
    if (predicate.identifier_like.is_some()
        || predicate.non_blank.is_some()
        || predicate.starts_with.is_some()
        || !predicate.one_of.is_empty())
        && matches!(
            predicate.kind,
            Some(
                ValueKind::Number
                    | ValueKind::Bool
                    | ValueKind::Null
                    | ValueKind::Array
                    | ValueKind::Object
            )
        )
    {
        return Some(
            "a text condition - `identifier_like`, `non_blank`, `starts_with`, `one_of` - needs a \
                 string, so beside a kind that is not one it can never hold",
        );
    }
    None
}

/// Every predicate set a compiled rule holds, wherever the declaration put it.
pub(super) fn predicate_sets(rule: &CompiledMessageRule) -> Vec<&ValueCondition> {
    let mut out = vec![&rule.raw_where];
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
    if opaque {
        return Condition {
            gate,
            narrowed: true,
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
    // nothing on a span whose text it refuses - which is a claim narrowed by the payload, like a reading's.
    Condition {
        gate,
        narrowed: (any && every_reading_narrowed) || !rule.raw_where.is_empty(),
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
    if rendered.contains('?') || rendered.contains('\\') {
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
/// Two independent facets, because a rule can have both and they answer different questions. The gate says
/// *which spans* the rule runs on and can be related to another rule's gate; `narrowed` says the rule may
/// read nothing even where it runs, for a reason nothing here can compare with another rule's.
#[derive(Clone, Default)]
pub(super) struct Condition {
    /// The spans this runs on. `None` means every span.
    pub(super) gate: Option<SpanExpr>,
    /// Narrowed by the payload, by a parent, or by which spelling of the carrier a producer used.
    pub(super) narrowed: bool,
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
