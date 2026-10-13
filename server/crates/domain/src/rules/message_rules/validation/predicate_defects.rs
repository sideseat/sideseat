//! The defects of a value predicate: a condition that can never hold where it is asserted, or that ignores one
//! of its parts.

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
            + usize::from(p.same_as.is_some())
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

/// The defects of a span condition's member predicate, at the polarity the condition sits at.
pub(in crate::rules) fn member_predicate_defect(
    predicate: &ValuePredicate,
    positive: bool,
) -> Option<&'static str> {
    atom_defect(predicate, positive)
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
    // A value always equals itself, so `same_as` naming its own *singular* path states nothing beyond presence. A
    // path that can select several values is different: compared with itself it asks that it select exactly one,
    // a real constraint. Beside `exists: false` it asks an absent value to equal another, which never holds.
    if let Some(other) = &predicate.same_as {
        let own = predicate
            .path
            .as_ref()
            .map_or_else(|| "$".to_string(), ToString::to_string);
        let singular = !own.contains(['*', '?', ':', ',']) && !own.contains("..");
        if other.to_string() == own && singular {
            return Some("`same_as` names the predicate's own path, which every value equals");
        }
        if predicate.exists == Some(false) {
            return Some(
                "`same_as` asks the value to equal another, which an absent value never does",
            );
        }
    }
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
    // A literal of one kind beside a `kind` of another: `{kind: object, equals: 1}` holds for nothing.
    if let (Some(kind), Some(literal)) = (predicate.kind, &predicate.equals)
        && kind_of(literal) != kind
    {
        return Some("`equals` names a value of another kind than `kind`, so no value is both");
    }
    // `only_members` asks about an object's members, so beside any other kind it can never hold.
    if !predicate.only_members.is_empty()
        && predicate.kind.is_some_and(|kind| kind != ValueKind::Object)
    {
        return Some(
            "`only_members` needs an object, so beside a kind that is not one it can never hold",
        );
    }
    None
}

/// The kind of a JSON literal.
fn kind_of(value: &serde_json::Value) -> ValueKind {
    match value {
        serde_json::Value::Null => ValueKind::Null,
        serde_json::Value::Bool(_) => ValueKind::Bool,
        serde_json::Value::Number(_) => ValueKind::Number,
        serde_json::Value::String(_) => ValueKind::String,
        serde_json::Value::Array(_) => ValueKind::Array,
        serde_json::Value::Object(_) => ValueKind::Object,
    }
}

/// A `raw_where` asserted about something other than text: it is asked of the carrier's raw string
/// (`emit::run`), before anything is parsed, so a root condition needing another kind - or a member of one - can
/// never hold.
pub(in crate::rules) fn raw_where_defect(condition: &ValueCondition) -> Option<&'static str> {
    fn defect(
        expr: &crate::rules::expr::Expr<ValuePredicate>,
        positive: bool,
    ) -> Option<&'static str> {
        use crate::rules::expr::Expr;
        match expr {
            Expr::Atom(predicate) if positive => {
                let on_root = predicate
                    .path
                    .as_ref()
                    .is_none_or(|path| path.to_string() == "$");
                let needs_structure = !on_root
                    || predicate.kind.is_some_and(|kind| kind != ValueKind::String)
                    || !predicate.only_members.is_empty()
                    || predicate
                        .equals
                        .as_ref()
                        .is_some_and(|literal| !literal.is_string());
                needs_structure.then_some(
                    "`raw_where` is asked of the carrier's raw text, so a condition needing another kind, or a \
                     member of a parsed value, can never hold - ask it in `where`, after `parse`",
                )
            }
            Expr::Atom(_) => None,
            Expr::All(group) | Expr::Any(group) => group
                .children()
                .iter()
                .find_map(|child| defect(child, positive)),
            Expr::Not(child) => defect(child, !positive),
        }
    }
    condition.declared().and_then(|expr| defect(expr, true))
}
