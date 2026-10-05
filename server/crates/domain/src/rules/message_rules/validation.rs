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
pub(in crate::rules) fn predicate_defect(set: &PredicateSet) -> Option<&'static str> {
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
            + usize::from(p.not_null.is_some())
            + usize::from(p.identifier_like.is_some())
            + usize::from(p.starts_with.is_some())
            + usize::from(p.lacks_prefix.is_some())
            + usize::from(!p.one_of.is_empty())
            + usize::from(!p.none_of.is_empty())
            == 1
    };
    let complements = |a: &ValuePredicate, b: &ValuePredicate| -> bool {
        sole(a)
            && sole(b)
            && ((a.not_null == Some(true) && b.not_null == Some(false))
                || (a.not_null == Some(false) && b.not_null == Some(true))
                || (a.non_empty == Some(true) && b.non_empty == Some(false))
                || (a.non_empty == Some(false) && b.non_empty == Some(true))
                || (a.identifier_like == Some(true) && b.identifier_like == Some(false))
                || (a.identifier_like == Some(false) && b.identifier_like == Some(true))
                // A prefix and its negation: every string has it or lacks it, and a non-string lacks it.
                || (a.starts_with.is_some() && a.starts_with == b.lacks_prefix)
                || (b.starts_with.is_some() && b.starts_with == a.lacks_prefix))
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
    if let Some(required) = all_root_kinds.first()
        && !any_root.is_empty()
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
            || (!any_root.is_empty() && any_root.iter().all(|p| p.not_null == Some(false))))
    {
        return Some(
            "an `all` set requires a root kind that is not null while a required branch asserts the \
                 root is null, so it holds for nothing",
        );
    }
    for (i, left) in all_root.iter().enumerate() {
        for right in &all_root[i + 1..] {
            if complements(left, right) {
                return Some(
                    "an `all` set holds a root condition and its negation, so it holds for nothing",
                );
            }
        }
    }

    for predicate in set.all.iter().chain(set.any.iter()) {
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
        if predicate.exists == Some(false)
            && (predicate.kind.is_some()
                || predicate.non_empty.is_some()
                || predicate.not_null.is_some()
                || predicate.identifier_like.is_some()
                || predicate.starts_with.is_some()
                || predicate.lacks_prefix.is_some()
                // `one_of` needs a value to be one of them, so it cannot hold on an absent member - and the
                // absent branch returns before consulting it, so it was silently ignored. `none_of` is
                // deliberately not here: its documented reading accepts absence, which is how a dialect's
                // unnamed events fall through to the reading that handles them.
                || !predicate.one_of.is_empty())
        {
            return Some(
                "`exists: false` asserts the member is absent, so no other condition on it \
                 can hold",
            );
        }
        // Only *overlapping* prefixes contradict. `starts_with: "ab"` with `lacks_prefix: "a"` cannot hold,
        // and so can `lacks_prefix: "ab"` with `starts_with: "a"` - but `starts_with: "a"` beside
        // `lacks_prefix: "b"` is an ordinary, satisfiable statement, and refusing it refused a real rule.
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
        // A predicate on the **root** that asserts nothing beyond presence is a tautology: the value being
        // tested always exists. `{}`, `{"path": "$"}` and `{"exists": true}` are the same statement, and each
        // makes a rule that requires it recognise everything. A member *path* with no conditions is
        // different and stays legal - it asserts the member is there.
        let on_root = predicate
            .path
            .as_ref()
            .is_none_or(|path| path.to_string() == "$");
        // The root always exists, so asserting its absence can never hold. Judged *before* the
        // presence-only rule below, which any other condition - `none_of`, say - would otherwise mask.
        if on_root && predicate.exists == Some(false) {
            return Some(
                "`exists: false` on the root can never hold - the value being tested is always there",
            );
        }
        let asserts_only_presence = predicate.kind.is_none()
            && predicate.non_empty.is_none()
            && predicate.not_null.is_none()
            && predicate.identifier_like.is_none()
            && predicate.starts_with.is_none()
            && predicate.lacks_prefix.is_none()
            && predicate.one_of.is_empty()
            && predicate.none_of.is_empty();
        if matches!(predicate.kind, Some(ValueKind::Null)) && predicate.not_null == Some(true) {
            return Some("`kind: null` and `not_null: true` on one predicate");
        }
        if predicate.kind.is_some()
            && !matches!(predicate.kind, Some(ValueKind::Null))
            && predicate.not_null == Some(false)
        {
            return Some("a kind that is not null, beside `not_null: false`");
        }
        if let Some(both) = predicate
            .one_of
            .iter()
            .find(|value| predicate.none_of.contains(value))
        {
            let _ = both;
            return Some("a value named by both `one_of` and `none_of`");
        }
        if on_root && asserts_only_presence {
            return Some(
                "a predicate on the root that asserts nothing beyond presence is a tautology - the \
                     value being tested always exists, so the condition recognises everything",
            );
        }
        // Every text condition needs a string. Declared beside a kind that is not one, it can never hold -
        // and a predicate that can never hold is the same defect as one that asserts nothing.
        if (predicate.identifier_like.is_some()
            || predicate.starts_with.is_some()
            || predicate.lacks_prefix.is_some()
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
                "a text condition - `identifier_like`, `starts_with`, `lacks_prefix`, `one_of` - needs \
                     a string, so beside a kind that is not one it can never hold",
            );
        }
    }
    None
}

/// Every predicate set a compiled rule holds, wherever the declaration put it.
pub(super) fn predicate_sets(rule: &CompiledMessageRule) -> Vec<&PredicateSet> {
    let mut out = Vec::new();
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
    if let Some(require) = &rule.read.entry_require {
        out.push(require);
    }
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
pub(super) fn wrap_predicate_sets(wrap: &WrapSpec) -> Vec<&PredicateSet> {
    let mut out: Vec<&PredicateSet> = wrap.attach.iter().map(|attach| &attach.require).collect();
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
/// Why a message-rule gate could never hold.
///
/// One definition for `when`, `unless` and a compose member's fallback: the dimensions a message gate is not
/// given, plus every way a gate is undeclarable whoever asks it.
pub(super) fn message_gate_defect(gate: &DetectMatch) -> Option<&'static str> {
    if super::detect_rules::unavailable_gate_dimension(gate).is_some() {
        return Some(
            "uses a resource dimension, and a message gate is given no resource attributes - it could never \
             hold",
        );
    }
    super::detect_rules::gate_defect(gate)
}

/// Whether a rule's claim on a carrier is *conditional* - on the span, or on nothing else having
/// supplied the value.
///
/// This is what separates a genuine conflict from a shared carrier. The check exists to catch a rule that
/// can never emit, and a rule whose claim is conditional is not that: it fires on the spans its condition
/// admits and yields to the other elsewhere, with `legacy_rank` deciding which is tried first. Two
/// *unconditional* rules on one carrier really are a defect - the second could never run.
///
/// Reachable in practice: the generic `output.value` is read by one dialect as a gated last resort and by
/// another as its own output, and they are told apart by the span they are on.
pub(super) fn rule_condition(rule: &CompiledMessageRule) -> Condition {
    // Both facets, not one label. A rule can be gated *and* payload-narrowed, and folding them into one
    // value made "same gate, mutually exclusive payloads" look like a dead pair while it is a working one.
    let gate = rule.when.as_ref().map(|g| g.match_spec.clone());
    // `unless` narrows in the opposite direction: this rule runs where the gate does *not* hold, and nothing
    // here can relate that to another rule's positive gate. Treated as an incomparable narrowing.
    // An instrumentation scope is a telemetry-envelope gate evaluated beside `when`. The carrier is free
    // outside that exact scope, so treating the rule as unconditional makes a producer-specific decoder
    // appear to permanently suppress the convention's general reading of the same attribute.
    let opaque = rule.unless.is_some() || rule.instrumentation_scope.is_some();
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
    Condition {
        gate,
        narrowed: any && every_reading_narrowed,
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
    match rule.read.first_present.as_slice() {
        // One spelling is not a choice.
        [only] if rule.read.attribute.is_none() => Some(only.as_str()),
        [] => rule.read.attribute.as_deref(),
        _ => None,
    }
}

/// The carriers a rule's emission owns **together**, where losing one loses the whole emission.
///
/// Empty for an ordinary reading, whose emission owns the one carrier it read - there, a lower-ranked rule
/// taking that carrier means the two take turns, which is what ranks are for. Non-empty for a reading that is
/// *all or nothing*, where an emission is accepted only if its whole ownership set is free:
///
/// | Reading | Owned together |
/// | --- | --- |
/// | `compose` with several members | every spelling of every member - `composed()` selects the **first present** with `find_map` and never retries a backup |
/// | `indexed_family` | the family's keys: each entry owns its own members, and an aggregate owns every entry |
/// | `overlay` | the base carrier and the overlay's, which are joined into one observation |
///
/// The distinction matters because the *conditional* excuse - "a gated rule and an ungated one take turns, and
/// the ranks decide" - is sound for a single-carrier reading and false here. A rule that takes one member of a
/// composed reading does not merely go first: the composed emission is dropped whole, so the carriers the
/// taker never wanted end up owned by **nobody** and their content disappears from the feed. Silently, and
/// neither rule looks wrong on its own.
pub(super) fn owned_all_or_nothing(rule: &CompiledMessageRule) -> Vec<CarrierPattern> {
    if let Some(set) = &rule.branch_set {
        // A branch leaf is a rule of its own, and each leaf's reading is all-or-nothing on its own terms.
        return set
            .primary
            .iter()
            .chain(&set.fallback)
            .chain(&set.always)
            .flat_map(owned_all_or_nothing)
            .collect();
    }
    let mut out: Vec<CarrierPattern> = Vec::new();
    if let Some(compose) = &rule.compose {
        // Only a *several*-member compose: one member reading several spellings takes exactly one of them, so
        // there is nothing another rule can take half of.
        if compose.members.len() > 1 {
            for member in &compose.members {
                out.extend(
                    member
                        .spec
                        .from_any_of
                        .iter()
                        .map(|key| CarrierPattern::Exact(key.clone())),
                );
            }
        }
    }
    if let Some(family) = &rule.read.indexed_family {
        out.push(CarrierPattern::Prefix(format!("{family}.")));
    }
    if let Some(overlay) = &rule.read.overlay {
        out.push(CarrierPattern::Exact(overlay.from.clone()));
        if let Some(attribute) = &rule.read.attribute {
            out.push(CarrierPattern::Exact(attribute.clone()));
        }
    }
    out
}

/// What narrows a claim on a carrier: the span it runs on, and whether the payload narrows it further.
///
/// Two independent facets, because a rule can have both and they answer different questions. The gate says
/// *which spans* the rule runs on and can be related to another rule's gate; `narrowed` says the rule may
/// read nothing even where it runs, for a reason nothing here can compare with another rule's.
#[derive(Clone, Default)]
pub(super) struct Condition {
    /// The spans this runs on. `None` means every span.
    pub(super) gate: Option<DetectMatch>,
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

/// Whether every span `narrower` admits is also admitted by `wider`.
///
/// Signal by signal, and only where one literally subsumes another - a longer span-name prefix, the same
/// attribute key, a shorter `contains` needle. Sound and deliberately incomplete: a missed subsumption
/// under-refuses, which leaves a dead rule to the corpus measurement, while a wrong one deletes a working
/// rule at startup.
pub(super) fn gate_covers(wider: &DetectMatch, narrower: &DetectMatch) -> bool {
    // Dimensions this cannot relate at all. Present on either side, nothing is provable - and saying so is
    // what keeps a missed subsumption an under-refusal rather than a deleted rule.
    if narrower.text_contains.is_some()
        || wider.text_contains.is_some()
        || !narrower.attr_equals_ignore_case.is_empty()
        || !wider.attr_equals_ignore_case.is_empty()
    {
        return false;
    }
    let all_covered = |them: &[String], us: &[String], subsumes: fn(&str, &str) -> bool| {
        them.iter()
            .all(|theirs| us.iter().any(|ours| subsumes(ours, theirs)))
    };
    let pairs_covered = |them: &[KeyValue], us: &[KeyValue], subsumes: fn(&str, &str) -> bool| {
        them.iter().all(|theirs| {
            us.iter()
                .any(|ours| ours.key == theirs.key && subsumes(&ours.value, &theirs.value))
        })
    };
    // A span-name signal matches by equality *or* prefix, so a longer needle is covered by a shorter one.
    let prefix_of = |ours: &str, theirs: &str| theirs.starts_with(ours);
    // A `contains` needle covers any needle that contains it.
    let inside = |ours: &str, theirs: &str| theirs.contains(ours);

    let any_signal = !narrower.span_name.is_empty()
        || !narrower.attr_prefix.is_empty()
        || !narrower.attr_equals.is_empty()
        || !narrower.attr_exists.is_empty()
        || !narrower.service_name.is_empty()
        || !narrower.span_attr_contains.is_empty()
        || !narrower.resource_attr_contains.is_empty();

    // Across dimensions where one runtime signal *implies* another. `attr_equals(k, v)` reads the key, so it
    // cannot hold unless `attr_exists(k)` does; and `attr_prefix(p)` holds of any key starting with `p`, so an
    // exact key that starts with `p` implies it. Same-dimension comparison alone let a wide `attr_exists`
    // rule sit ahead of a narrow `attr_equals` one on the same carrier, where the second can never own it.
    let key_covered = |key: &str| {
        wider.attr_exists.iter().any(|ours| ours == key)
            || wider
                .attr_prefix
                .iter()
                .any(|prefix| key.starts_with(prefix.as_str()))
    };
    let equals_covered = narrower.attr_equals.iter().all(|theirs| {
        key_covered(&theirs.key)
            || wider
                .attr_equals
                .iter()
                .any(|ours| ours.key == theirs.key && ours.value == theirs.value)
    });
    let exists_covered = narrower
        .attr_exists
        .iter()
        .all(|theirs| key_covered(theirs));

    any_signal
        && all_covered(&narrower.span_name, &wider.span_name, prefix_of)
        && all_covered(&narrower.attr_prefix, &wider.attr_prefix, prefix_of)
        && exists_covered
        && all_covered(&narrower.service_name, &wider.service_name, inside)
        && equals_covered
        && pairs_covered(
            &narrower.span_attr_contains,
            &wider.span_attr_contains,
            inside,
        )
        && pairs_covered(
            &narrower.resource_attr_contains,
            &wider.resource_attr_contains,
            inside,
        )
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
    if let Some(attribute) = rule.read.attribute.as_deref() {
        out.push(always(CarrierPattern::Exact(attribute.to_string())));
    }
    // Only the *first* alternative is claimed unconditionally: the rest are read where no earlier spelling
    // was present, so a rule reading a later one yields whenever an earlier one is there.
    for (position, key) in rule.read.first_present.iter().enumerate() {
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
        out.push(
            if rule.require_members.is_some() || rule.read.entry_require.is_some() {
                only_sometimes(pattern)
            } else {
                always(pattern)
            },
        );
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
    if let Some(attribute) = rule.read.attribute.as_deref() {
        out.push(always(CarrierPattern::Exact(attribute.to_string())));
    }
    // A tag per spelling, and only the first is emitted whatever the span carries.
    for key in &rule.read.each {
        out.push(always(CarrierPattern::Exact(key.clone())));
    }
    for (position, key) in rule.read.first_present.iter().enumerate() {
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
        out.push(
            if rule.require_members.is_some() || rule.read.entry_require.is_some() {
                Consumed {
                    pattern,
                    condition: Condition {
                        gate: None,
                        narrowed: true,
                    },
                }
            } else {
                always(pattern)
            },
        );
    }
    out
}
