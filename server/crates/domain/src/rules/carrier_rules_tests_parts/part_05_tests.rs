/// Predicates that hold when they should not, and declarations that could never hold.
///
/// Each of these compiled or held before the format review, and each is the same failure at predicate level:
/// a condition that reads as narrow and matches everything, or reads as a requirement and asks for nothing.
#[test]
fn a_predicate_that_could_never_mean_what_it_says_is_refused() {
    use crate::rules::schema::RuleFile;

    // `KeyValue` was the one predicate type without strict decoding, so a flag on it was **discarded** and
    // the comparison an author asked to be case-insensitive stayed case-sensitive.
    let with_stray_member = serde_json::from_value::<RuleFile>(serde_json::json!({
        "id": "probe",
        "detect": [{
            "id": "probe.detect",
            "label": "probe",
            "priority": 1,
            "match": {"attr_equals": [{"key": "span.kind", "value": "TOOL", "ignore_case": true}]},
        }],
    }));
    assert!(
        with_stray_member.is_err(),
        "a stray member on a key/value predicate must be refused, or the flag is silently discarded"
    );

    // An empty substring: every present value contains it, so the rule matches every span carrying the key.
    // Refused for both detection and a gate, through one validator - they had drifted apart in both
    // directions, so this asserts the *same* answer from both callers.
    let contains_nothing = serde_json::json!({
        "span_attr_contains": [{"key": "metadata", "value": ""}],
    });
    let spec: crate::rules::schema::DetectMatch =
        serde_json::from_value(contains_nothing).expect("the probe parses");
    assert!(
        crate::rules::detect_rules::atom_literal_defect(&spec).is_some(),
        "an empty substring search must be refused: every present value contains it"
    );

    // An equality against the empty string stays legal - a producer can write an attribute that holds it.
    let equals_empty: crate::rules::schema::DetectMatch =
        serde_json::from_value(serde_json::json!({"attr_equals": [{"key": "k", "value": ""}]}))
            .expect("the probe parses");
    assert!(
        crate::rules::detect_rules::atom_literal_defect(&equals_empty).is_none(),
        "an attribute that holds the empty string is a thing a producer writes"
    );

    // A member requirement naming nothing asks for `<entry>.`, so the rule is dead; a requirement with no
    // entries holds for every entry, which is the opposite of "at least one of these".
    let probe_family = |require: serde_json::Value| {
        let asset = serde_json::json!({
            "id": "probe",
            "messages": [{
                "id": "probe.family",
                "priority": 1,
                "read": {"indexed_family": "probe.items"},
                "emit": "message",
                "require_members": require,
            }],
        });
        crate::rules::message_rules::compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "probe.json".to_string(),
                serde_json::to_vec(&asset).expect("the probe serialises"),
            )]))
            .expect("the probe assets parse"),
        )
    };
    assert!(
        probe_family(serde_json::json!({"all_of": [{"name": ""}]})).is_err(),
        "a requirement naming an empty member asks for `<entry>.` and can never hold"
    );
    assert!(
        probe_family(serde_json::json!({})).is_err(),
        "a requirement with no entries holds for every entry, which is not a requirement"
    );
    assert!(
        probe_family(serde_json::json!({"all_of": [{"name": "role"}]})).is_ok(),
        "a requirement naming a member must compile"
    );
}

/// `non_empty` asks a question a scalar cannot answer, so a scalar fails it either way.
///
/// `{"path": "$.content", "non_empty": true}` held for `{"content": 0}` - a number reported as filled
/// content. The compiler's refusal does not cover it: that one is about the *declaration*, and this is about
/// the value that arrives.
#[test]
fn a_scalar_is_neither_empty_nor_non_empty() {
    use crate::rules::message_rules::predicate_holds_for_test as holds;

    for (subject, want) in [
        (serde_json::json!("text"), true),
        (serde_json::json!(""), false),
        (serde_json::json!([1]), true),
        (serde_json::json!([]), false),
        (serde_json::json!({"a": 1}), true),
        (serde_json::json!({}), false),
        // Neither, whichever way it is asked.
        (serde_json::json!(0), false),
        (serde_json::json!(7), false),
        (serde_json::json!(true), false),
        (serde_json::json!(serde_json::Value::Null), false),
    ] {
        let predicate: crate::rules::schema::ValuePredicate =
            serde_json::from_value(serde_json::json!({"non_empty": true}))
                .expect("the probe parses");
        assert_eq!(
            holds(&predicate, &subject),
            want,
            "`non_empty: true` over {subject}"
        );
        let negated: crate::rules::schema::ValuePredicate =
            serde_json::from_value(serde_json::json!({"non_empty": false}))
                .expect("the probe parses");
        // A string, array or object answers the negation; a scalar still fails, because the vocabulary has
        // nothing to say about it.
        let scalar = matches!(
            subject,
            serde_json::Value::Number(_) | serde_json::Value::Bool(_) | serde_json::Value::Null
        );
        assert_eq!(
            holds(&negated, &subject),
            !scalar && !want,
            "`non_empty: false` over {subject}"
        );
    }
}

/// The boolean grammar answers exactly as the shell it replaces, over generated spans.
///
/// The migration's whole risk is that a translated condition means something slightly different, and the
/// difference shows only on an input nobody wrote a fixture for. So this compares the new expression against
/// the retired `DetectMatch` evaluator on **every** `DetectMatch` the shipped assets contain, over a generated
/// set of spans chosen to sit on each dimension's boundary: the attribute present with the value, present with
/// another value, absent entirely, and the span name matching or not.
///
/// `Unknown` is where they may legitimately differ, and the assertion is directional: where the old evaluator
/// said **true**, the new one must say true. The old one could not say "I cannot ask this", so it answered
/// false for an absent attribute - and the new one answers `Unknown`, which also does not hold. Requiring
/// equality of `holds()` is therefore the right comparison, and it is the one made here.
#[test]
fn the_boolean_grammar_answers_as_the_shell_it_replaces() {
    use crate::rules::expr::{SpanSubject, span_expr_of};
    use crate::rules::schema::{DetectMatch, RuleFile};
    use std::collections::HashMap;

    // Every `DetectMatch` the assets declare, wherever it sits.
    let mut specs: Vec<(String, DetectMatch)> = Vec::new();
    for (path, bytes) in crate::rules::schema::embedded_sources() {
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("the asset parses");
        let file: RuleFile = serde_json::from_slice(&bytes).expect("the asset parses");
        for rule in &file.detect {
            specs.push((format!("{path}:{}", rule.id), rule.match_spec.clone()));
        }
        for rule in file.observation_types.iter().chain(&file.span_categories) {
            for (index, conjunct) in rule.all_of.iter().enumerate() {
                specs.push((format!("{path}:{}#{index}", rule.id), conjunct.clone()));
            }
        }
        // The gates, wherever they are nested: `when` and `unless` on a message rule, a branch leaf, a field
        // source, a compose fallback. Walked over the raw JSON so a nesting nobody remembered is included.
        fn walk(value: &serde_json::Value, path: &str, out: &mut Vec<(String, DetectMatch)>) {
            match value {
                serde_json::Value::Object(members) => {
                    for (key, inner) in members {
                        if (key == "when" || key == "unless")
                            && let Ok(spec) = serde_json::from_value::<DetectMatch>(inner.clone())
                        {
                            out.push((format!("{path}/{key}"), spec));
                        }
                        walk(inner, path, out);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        walk(item, path, out);
                    }
                }
                _ => {}
            }
        }
        walk(&value, &path, &mut specs);
    }
    assert!(
        specs.len() > 40,
        "only {} conditions were found in the assets, which cannot be right",
        specs.len()
    );

    // The keys and values every condition mentions, so the generated spans sit on their boundaries.
    let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut values: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (_, spec) in &specs {
        keys.extend(spec.attr_exists.iter().cloned());
        keys.extend(spec.attr_prefix.iter().cloned());
        for pair in spec
            .attr_equals
            .iter()
            .chain(&spec.attr_equals_ignore_case)
            .chain(&spec.span_attr_contains)
        {
            keys.insert(pair.key.clone());
            values.insert(pair.value.clone());
        }
        names.extend(spec.span_name.iter().cloned());
        if let Some(text) = &spec.text_contains {
            for source in &text.sources {
                if let Some(key) = source.strip_prefix("attr:") {
                    keys.insert(key.to_string());
                }
            }
            values.extend(text.needles.iter().cloned());
        }
    }

    let mut compared = 0_usize;
    let mut per_condition: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    let mut disagreements: Vec<String> = Vec::new();
    let mut untranslated: Vec<String> = Vec::new();
    for (id, spec) in &specs {
        // A condition that translates to nothing is **not** silently skipped. Skipping it was this test's own
        // version of the defect it exists to catch: dropping a dimension from the translation made every
        // condition that used only that dimension produce `None`, so it was skipped rather than compared, and
        // the mutation passed. A condition with any signal at all must translate.
        let has_signal = !spec.span_name.is_empty()
            || !spec.attr_prefix.is_empty()
            || !spec.attr_exists.is_empty()
            || !spec.attr_equals.is_empty()
            || !spec.attr_equals_ignore_case.is_empty()
            || !spec.span_attr_contains.is_empty()
            || spec
                .text_contains
                .as_ref()
                .is_some_and(|text| !text.needles.is_empty() && !text.sources.is_empty());
        let expr = match span_expr_of(spec) {
            Some(expr) => expr,
            None => {
                if has_signal {
                    untranslated.push(format!("  {id}: {spec:?}"));
                }
                continue;
            }
        };
        // Spans built from **this condition's own** literals, not from a global cross product truncated to a
        // budget. That was the first form and it hid two mutations: the targeted inputs were appended after a
        // large product and then cut off by the `take`, so a case-folding change and an ignored
        // `first_present` flag were never reached. A per-condition set is smaller *and* complete.
        let mut spans: Vec<(String, HashMap<String, String>)> =
            vec![("other.span".to_string(), HashMap::new())];
        let mut mentioned_keys: Vec<String> = spec
            .attr_exists
            .iter()
            .chain(&spec.attr_prefix)
            .cloned()
            .collect();
        // A key *under* each declared prefix, since that dimension is about the key rather than the value and
        // an exactly-equal key is not what it asks.
        for prefix in &spec.attr_prefix {
            mentioned_keys.push(format!("{prefix}something"));
        }
        let mut mentioned_values: Vec<String> = Vec::new();
        for pair in spec
            .attr_equals
            .iter()
            .chain(&spec.attr_equals_ignore_case)
            .chain(&spec.span_attr_contains)
        {
            mentioned_keys.push(pair.key.clone());
            mentioned_values.push(pair.value.clone());
        }
        if let Some(text) = &spec.text_contains {
            for source in &text.sources {
                if let Some(key) = source.strip_prefix("attr:") {
                    mentioned_keys.push(key.to_string());
                }
            }
            mentioned_values.extend(text.needles.iter().cloned());
        }
        mentioned_keys.sort();
        mentioned_keys.dedup();
        mentioned_values.sort();
        mentioned_values.dedup();
        // A condition may mention keys and no values at all - `attr_exists`, `attr_prefix`, a bare span-name
        // prefix. Those still need a span where the key is there and one where it is not, or the condition is
        // compared only against the empty span and the comparison shows nothing.
        if mentioned_values.is_empty() {
            mentioned_values.push("a value".to_string());
            mentioned_values.push(String::new());
        }
        if mentioned_keys.is_empty() {
            mentioned_keys.push("some.attribute".to_string());
        }

        let flip = |text: &str| -> String {
            text.chars()
                .map(|c| {
                    if c.is_ascii_lowercase() {
                        c.to_ascii_uppercase()
                    } else {
                        c.to_ascii_lowercase()
                    }
                })
                .collect()
        };

        let span_names: Vec<String> = spec
            .span_name
            .iter()
            .cloned()
            .chain(["other.span".to_string()])
            .collect();
        for name in &span_names {
            // The key absent altogether, which is where a value question has no answer.
            spans.push((name.clone(), HashMap::new()));
            for key in &mentioned_keys {
                for value in &mentioned_values {
                    for variant in [
                        value.clone(),
                        flip(value),
                        format!("prefix {value} suffix"),
                        "a value nothing mentions".to_string(),
                        String::new(),
                    ] {
                        let mut attrs = HashMap::new();
                        attrs.insert(key.clone(), variant);
                        spans.push((name.clone(), attrs));
                    }
                }
            }
            // The case that separates `first_present` from `any_present`, which needs *different* values
            // across the keys: exactly one source holds the matching value and the others hold something
            // unrelated. With the match on a later source, a first-present search says no and an any-present
            // search says yes - and with every key holding the same value the two agree, which is why the
            // same-value spans below could not see an ignored flag.
            if mentioned_keys.len() > 1 {
                for value in mentioned_values.iter().take(2) {
                    for matching in &mentioned_keys {
                        let mut attrs = HashMap::new();
                        for key in &mentioned_keys {
                            attrs.insert(
                                key.clone(),
                                if key == matching {
                                    value.clone()
                                } else {
                                    "unrelated".to_string()
                                },
                            );
                        }
                        spans.push((name.clone(), attrs));
                    }
                }
            }
            // Every key present at once, and each *single* key present with the others absent.
            if mentioned_keys.len() > 1 {
                for value in mentioned_values.iter().take(2) {
                    let mut all = HashMap::new();
                    for key in &mentioned_keys {
                        all.insert(key.clone(), value.clone());
                    }
                    spans.push((name.clone(), all));
                    for present in &mentioned_keys {
                        let mut one = HashMap::new();
                        one.insert(present.clone(), value.clone());
                        spans.push((name.clone(), one));
                        let mut one_other = HashMap::new();
                        one_other.insert(present.clone(), "unrelated".to_string());
                        spans.push((name.clone(), one_other));
                    }
                }
            }
        }
        for (span_name, attrs) in &spans {
            let old = crate::rules::detect_rules::signals_hold_for_test(spec, span_name, attrs);
            let new = expr
                .eval(&mut |atom| atom.eval(&SpanSubject { span_name, attrs }))
                .holds();
            compared += 1;
            *per_condition.entry(id.clone()).or_default() += 1;
            if old != new {
                disagreements.push(format!(
                    "  {id}: span `{span_name}` attrs {attrs:?} - retired said {old}, the grammar says {new}"
                ));
            }
        }
    }
    assert!(
        untranslated.is_empty(),
        "{} condition(s) have a signal and translate to no expression, so they were never compared - which \
         is how a dropped dimension passes this test:\n{}",
        untranslated.len(),
        untranslated.join("\n")
    );
    // A floor per **condition**, not a total. A large total of arbitrary spans is what the first form had, and
    // it reached none of the boundaries that matter; what makes a comparison worth counting is that it was
    // built from the condition's own literals, so the requirement is that every condition got several.
    let thin: Vec<&String> = per_condition
        .iter()
        .filter(|(_, count)| **count < 3)
        .map(|(id, _)| id)
        .collect();
    assert!(
        thin.is_empty(),
        "{} condition(s) were compared on fewer than three spans, so the translation is barely exercised \
         for them: {:?}",
        thin.len(),
        thin.iter().take(8).collect::<Vec<_>>()
    );
    assert!(
        compared > 500,
        "only {compared} comparisons were made in total, which cannot exercise the translation"
    );
    disagreements.sort();
    disagreements.dedup();
    assert!(
        disagreements.is_empty(),
        "{} of {compared} comparisons disagree. The translation must preserve meaning exactly, or a \
         migrated condition means something the asset did not say:\n{}",
        disagreements.len(),
        disagreements
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The JSON half of the grammar answers as the shell it replaces, including the witness binding.
///
/// The binding is the finding this test exists for. A `ValuePredicate` carries a path and several conditions,
/// and the retired evaluator required **one selected value** to satisfy them all - so
/// `{path: "$.items[*]", starts_with: "a", one_of: ["apple","banana"]}` is false for `["avocado","banando"]`.
/// Translating each condition into its own atom under `all` makes that *true*, with different elements
/// witnessing the two clauses: a silent change of meaning in every multi-condition rule. So the first case
/// below is the minimal counterexample; the rest covers every predicate the assets declare over generated payloads.
#[test]
fn the_json_grammar_answers_as_the_shell_it_replaces() {
    use crate::rules::expr::json_expr_of_predicate;
    use crate::rules::message_rules::predicate_holds_for_test as retired;
    use crate::rules::schema::ValuePredicate;

    let evaluate = |predicate: &ValuePredicate, payload: &serde_json::Value| -> (bool, bool) {
        let old = retired(predicate, payload);
        let new = json_expr_of_predicate(predicate)
            .map(|expr| expr.eval(&mut |atom| atom.eval(payload)).holds())
            // A predicate stating nothing translates to nothing; the retired evaluator held for it.
            .unwrap_or(true);
        (old, new)
    };

    // The witness case, named explicitly so a regression says what broke.
    let witness: ValuePredicate = serde_json::from_value(serde_json::json!({
        "path": "$.items[*]",
        "starts_with": "a",
        "one_of": ["apple", "banana"],
    }))
    .expect("the probe parses");
    let split = serde_json::json!({"items": ["avocado", "banana"]});
    let (old, new) = evaluate(&witness, &split);
    assert!(
        !old,
        "the retired evaluator required one value to satisfy every condition"
    );
    assert_eq!(
        new, old,
        "two different elements must not witness two clauses of one predicate - the conditions share a \
         selection, which is why a predicate becomes one `some` holding a sub-expression rather than several \
         atoms under `all`"
    );
    // And a payload where one element does satisfy both must still hold.
    let together = serde_json::json!({"items": ["apple", "cherry"]});
    let (old, new) = evaluate(&witness, &together);
    assert!(old && new, "one element satisfying both must hold");

    // Every predicate the assets declare, over payloads generated from its own literals.
    let mut predicates: Vec<(String, ValuePredicate)> = Vec::new();
    for (path, bytes) in crate::rules::schema::embedded_sources() {
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("the asset parses");
        fn walk(value: &serde_json::Value, path: &str, out: &mut Vec<(String, ValuePredicate)>) {
            match value {
                serde_json::Value::Object(members) => {
                    // A predicate set sits under many member names, so it is recognised by *shape*: an object
                    // with `all` or `any` holding objects that parse as predicates.
                    for key in ["all", "any"] {
                        if let Some(serde_json::Value::Array(items)) = members.get(key) {
                            for item in items {
                                if let Ok(predicate) =
                                    serde_json::from_value::<ValuePredicate>(item.clone())
                                {
                                    out.push((format!("{path}/{key}"), predicate));
                                }
                            }
                        }
                    }
                    for inner in members.values() {
                        walk(inner, path, out);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        walk(item, path, out);
                    }
                }
                _ => {}
            }
        }
        walk(&value, &path, &mut predicates);
    }
    assert!(
        predicates.len() > 30,
        "only {} predicates were found in the assets, which cannot be right",
        predicates.len()
    );

    let mut compared = 0_usize;
    let mut disagreements: Vec<String> = Vec::new();
    for (id, predicate) in &predicates {
        // Values this predicate mentions, plus the shapes that sit on its boundaries.
        let mut mentioned: Vec<serde_json::Value> = predicate
            .one_of
            .iter()
            .chain(&predicate.none_of)
            .map(|text| serde_json::json!(text))
            .collect();
        if let Some(prefix) = &predicate.starts_with {
            mentioned.push(serde_json::json!(prefix));
            mentioned.push(serde_json::json!(format!("{prefix}more")));
        }
        if let Some(prefix) = &predicate.lacks_prefix {
            mentioned.push(serde_json::json!(prefix));
            mentioned.push(serde_json::json!(format!("{prefix}more")));
        }
        mentioned.extend([
            serde_json::json!("something else"),
            serde_json::json!(""),
            serde_json::json!(0),
            serde_json::json!(7),
            serde_json::json!(true),
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!(["a"]),
            serde_json::json!({}),
            serde_json::json!({"k": "v"}),
        ]);

        // The path's own leading member, so a payload can actually place a value where it looks.
        let member = predicate
            .path
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default()
            .trim_start_matches("$.")
            .trim_start_matches("$['")
            .split(['.', '[', '\''])
            .next()
            .unwrap_or("value")
            .to_string();

        let mut payloads: Vec<serde_json::Value> = vec![
            serde_json::json!({}),
            serde_json::json!([]),
            serde_json::json!("a bare string"),
        ];
        for value in &mentioned {
            payloads.push(serde_json::json!({member.clone(): value.clone()}));
            payloads.push(value.clone());
            // A list at the member, so a plural path selects more than one - which is where the witness
            // binding shows.
            payloads.push(serde_json::json!({member.clone(): [value.clone()]}));
            for other in mentioned.iter().take(3) {
                payloads.push(serde_json::json!({member.clone(): [value.clone(), other.clone()]}));
            }
        }

        for payload in &payloads {
            let (old, new) = evaluate(predicate, payload);
            compared += 1;
            if old != new {
                disagreements.push(format!(
                    "  {id}: {predicate:?} over {payload} - retired said {old}, the grammar says {new}"
                ));
            }
        }
    }
    assert!(
        compared > 2000,
        "only {compared} comparisons were made, which cannot exercise the translation"
    );
    disagreements.sort();
    disagreements.dedup();
    assert!(
        disagreements.is_empty(),
        "{} of {compared} comparisons disagree:\n{}",
        disagreements.len(),
        disagreements
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The third truth value is observable, and this is where.
///
/// The translation oracle cannot see it: no *translated* expression negates a `some`, so `Unknown` and `False`
/// are indistinguishable through it - both fail `holds()`. Removing the empty-selection case therefore passed
/// that oracle. The distinction exists for expressions an author writes directly, where the whole point is that
/// `not` over "there was nothing to ask about" does **not** become "yes".
///
/// That is the trap this repository knows from SQL: `NOT IN (…)` over a nullable column drops the rows with no
/// value, and `Filter::positive_twin` exists because negating "is this session" dropped every trace with no
/// session. `none_of` is the same mistake at predicate level.
#[test]
fn a_negation_over_nothing_does_not_hold() {
    use crate::rules::expr::{Expr, JsonAtom, JsonSubjectAtom, Truth};

    // The constructors return `Option`, because an empty group is unconstructible by design. Every group here
    // is written with two children, so the invariant holds and these say so.
    fn two_all(children: Vec<Expr<JsonAtom>>) -> Expr<JsonAtom> {
        Expr::all(children).expect("two children were written out")
    }
    fn two_any(children: Vec<Expr<JsonAtom>>) -> Expr<JsonAtom> {
        Expr::any(children).expect("two children were written out")
    }
    use crate::rules::schema::{JsonPath, ValueKind};

    let path = JsonPath::parse("$.name").expect("a path");
    let payload = serde_json::json!({"other": "value"});
    let present = serde_json::json!({"name": "alice"});

    let some_is = |values: Vec<&str>| {
        Expr::Atom(JsonAtom::Some {
            path: path.clone(),
            satisfies: Box::new(Expr::Atom(JsonSubjectAtom::OneOf {
                values: values.into_iter().map(str::to_string).collect(),
            })),
        })
    };
    let eval =
        |expr: &Expr<JsonAtom>, value: &serde_json::Value| expr.eval(&mut |atom| atom.eval(value));

    // The member is absent: the question cannot be asked.
    assert_eq!(eval(&some_is(vec!["bob"]), &payload), Truth::Unknown);
    // And negating it stays Unknown, so the condition does not hold. Two-valued, this would be `true` - a rule
    // saying "the name is not bob" would match a payload with no name at all.
    assert_eq!(
        eval(&Expr::Not(Box::new(some_is(vec!["bob"]))), &payload),
        Truth::Unknown
    );
    assert!(
        !eval(&Expr::Not(Box::new(some_is(vec!["bob"]))), &payload).holds(),
        "a negation over an absent value must not hold"
    );

    // Present and not in the set: a real no, so the negation is a real yes.
    assert_eq!(eval(&some_is(vec!["bob"]), &present), Truth::False);
    assert!(eval(&Expr::Not(Box::new(some_is(vec!["bob"]))), &present).holds());

    // Presence itself is **total**, which is how absence is stated: asking about presence always has an answer,
    // so its negation is a real yes.
    let exists = Expr::Atom(JsonAtom::Exists { path: path.clone() });
    assert_eq!(eval(&exists, &payload), Truth::False);
    assert!(eval(&Expr::Not(Box::new(exists)), &payload).holds());

    // `all` and `any` are strong Kleene: a False child settles a conjunction whatever else is Unknown, and a
    // True child settles a disjunction. Without that, one unanswerable atom would poison a whole condition.
    let unknown = some_is(vec!["bob"]);
    let definitely_false = Expr::Atom(JsonAtom::Some {
        path: JsonPath::parse("$.other").expect("a path"),
        satisfies: Box::new(Expr::Atom(JsonSubjectAtom::Kind {
            kind: ValueKind::Number,
        })),
    });
    assert_eq!(
        eval(
            &two_all(vec![unknown.clone(), definitely_false.clone()]),
            &payload
        ),
        Truth::False
    );
    assert_eq!(
        eval(&two_any(vec![unknown.clone(), definitely_false]), &payload),
        Truth::Unknown
    );
    let definitely_true = Expr::Atom(JsonAtom::Some {
        path: JsonPath::parse("$.other").expect("a path"),
        satisfies: Box::new(Expr::Atom(JsonSubjectAtom::Kind {
            kind: ValueKind::String,
        })),
    });
    assert_eq!(
        eval(
            &two_any(vec![unknown.clone(), definitely_true.clone()]),
            &payload
        ),
        Truth::True
    );
    assert_eq!(
        eval(&two_all(vec![unknown, definitely_true]), &payload),
        Truth::Unknown
    );
}

/// A shared rank is refused where the order is observable, and allowed where it is not.
///
/// The tie-break was the rule **id**, so renaming a rule changed which of two contenders read a carrier. A rule
/// id must not be a control-flow primitive - and classification and detection already refuse a shared rank
/// outright, so message rules were the one resolver where it was silently policy.
///
/// Not refused globally, and the second half of this test is why: the five shared ranks in the shipped assets
/// each pair a *message* rule with a *metadata* one, whose orders are independent. Refusing those would force
/// five renumberings that state nothing.
#[test]
fn a_shared_message_rank_is_refused_only_where_the_order_shows() {
    let compiled = |rules: serde_json::Value| {
        // The events the probes select on have to be declared, or compilation refuses them for that reason
        // instead - which is a different refusal and would make this test pass for the wrong reason.
        let asset = serde_json::json!({
            "id": "probe",
            "message_events": [
                {"id": "probe.probe_first", "name": "probe.first"},
                {"id": "probe.probe_second", "name": "probe.second"},
                {"id": "probe.probe_shared", "name": "probe.shared"},
                {"id": "probe.probe_other", "name": "probe.other"},
            ],
            "messages": rules,
        });
        crate::rules::message_rules::compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "probe.json".to_string(),
                serde_json::to_vec(&asset).expect("the probe serialises"),
            )]))
            .expect("the probe assets parse"),
        )
    };

    // Two conditional message rules read one attribute at one rank. `a` wins today because
    // ids sort; renaming it to `zz` would hand the carrier to the other.
    let contending = serde_json::json!([
        {
            "id": "probe.a",
            "priority": 10,
            "read": {"attribute": "shared"},
            "parse": "text",
            "when": {"attr_exists": ["left"]},
            "emit": "message",
        },
        {
            "id": "probe.z",
            "priority": 10,
            "read": {"attribute": "other"},
            "parse": "text",
            "when": {"attr_exists": ["right"]},
            "emit": "message",
        },
    ]);
    assert!(
        compiled(contending).is_err(),
        "two message rules at one rank in the same stage must be refused: the tie-break is their ids"
    );

    // Different **axes**: a message and a tool definition are never read by the same path.
    let cross_axis = serde_json::json!([
        {
            "id": "probe.message",
            "priority": 10,
            "read": {"attribute": "one"},
            "parse": "text",
            "emit": "message",
        },
        {
            "id": "probe.tools",
            "priority": 10,
            "read": {"attribute": "two"},
            "parse": "json",
            "emit": "tool_definitions",
        },
    ]);
    assert!(
        compiled(cross_axis).is_ok(),
        "a message rule and a metadata rule at one rank order nothing relative to each other"
    );

    // Different **event domains**: selected by name, and the names do not intersect.
    let disjoint_events = serde_json::json!([
        {
            "id": "probe.one",
            "priority": 20,
            "source": {"event": {"names": ["probe.first"]}},
            "read": {"attribute": "one"},
            "parse": "text",
            "emit": "message",
        },
        {
            "id": "probe.two",
            "priority": 20,
            "source": {"event": {"names": ["probe.second"]}},
            "read": {"attribute": "two"},
            "parse": "text",
            "emit": "message",
        },
    ]);
    assert!(
        compiled(disjoint_events).is_ok(),
        "two event rules at one rank whose names never coincide are never candidates together"
    );

    // An event rule beside a **span** rule: one is selected by event name and the other reads a span's
    // attributes, so they are never candidates in the same pass.
    let event_beside_span = serde_json::json!([
        {
            "id": "probe.event",
            "priority": 25,
            "source": {"event": {"names": ["probe.first"]}},
            "read": {"attribute": "one"},
            "parse": "text",
            "emit": "message",
        },
        {
            "id": "probe.span",
            "priority": 25,
            "read": {"attribute": "two"},
            "parse": "text",
            "emit": "message",
        },
    ]);
    assert!(
        compiled(event_beside_span).is_ok(),
        "an event rule and a span rule at one rank are never candidates together"
    );

    // The same, where the names *do* intersect.
    let overlapping_events = serde_json::json!([
        {
            "id": "probe.one",
            "priority": 20,
            "source": {"event": {"names": ["probe.shared"]}},
            "read": {"attribute": "one"},
            "parse": "text",
            "emit": "message",
        },
        {
            "id": "probe.two",
            "priority": 20,
            "source": {"event": {"names": ["probe.shared", "probe.other"]}},
            "read": {"attribute": "two"},
            "parse": "text",
            "emit": "message",
        },
    ]);
    assert!(
        compiled(overlapping_events).is_err(),
        "two event rules at one rank sharing an event name contend on that event"
    );

    // Different **stages**: a fallback rule runs only where the dialect stage produced nothing.
    let cross_stage = serde_json::json!([
        {
            "id": "probe.dialect",
            "priority": 30,
            "read": {"attribute": "one"},
            "parse": "text",
            "emit": "message",
        },
        {
            "id": "probe.fallback",
            "priority": 30,
            "source": {"span": {"stage": "fallback"}},
            "read": {"attribute": "two"},
            "parse": "text",
            "emit": "message",
        },
    ]);
    assert!(
        compiled(cross_stage).is_ok(),
        "a fallback rule's rank orders nothing against a dialect rule's"
    );
}
