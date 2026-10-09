/// Predicates that hold when they should not, and declarations that could never hold.
///
/// Each of these compiled or held before the format review, and each is the same failure at predicate level:
/// a condition that reads as narrow and matches everything, or reads as a requirement and asks for nothing.
#[test]
fn a_predicate_that_could_never_mean_what_it_says_is_refused() {
    use crate::rules::schema::RuleFile;

    // A stray member on a condition is refused rather than discarded: `KeyValue` was once the one predicate type
    // without strict decoding, so a flag on it was dropped and a case-insensitive comparison stayed sensitive.
    let with_stray_member = serde_json::from_value::<RuleFile>(serde_json::json!({
        "id": "probe",
        "detect": [{
            "id": "probe.detect",
            "label": "probe",
            "priority": 1,
            "where": {"source": "attr:span.kind", "equals": "TOOL", "ignore_case": true},
        }],
    }));
    assert!(
        with_stray_member.is_err(),
        "a stray member on a key/value predicate must be refused, or the flag is silently discarded"
    );

    // An empty substring: every present value contains it, so the rule matches every span carrying the key.
    // Refused wherever a `where` is written, through one validator - detection and the gates had drifted apart
    // in both directions when they had two.
    let lowered = |condition: serde_json::Value| {
        let condition: crate::rules::schema::SpanWhere =
            serde_json::from_value(condition).expect("the probe parses");
        crate::rules::detect_rules::checked_condition(
            &condition,
            crate::rules::span_conditions::Readable::SPAN,
        )
    };
    assert!(
        lowered(serde_json::json!({"source": "attr:metadata", "contains": ""})).is_err(),
        "an empty substring search must be refused: every present value contains it"
    );
    // An equality against the empty string stays legal - a producer can write an attribute that holds it.
    assert!(
        lowered(serde_json::json!({"source": "attr:k", "equals": ""})).is_ok(),
        "an attribute that holds the empty string is a thing a producer writes"
    );
    // A source this section is not given is refused, so is a test a source cannot answer.
    assert!(
        lowered(serde_json::json!({"source": "resource:service.name", "contains": "x"})).is_err()
    );
    assert!(lowered(serde_json::json!({"source": "scope.name", "equals": "x"})).is_err());
    assert!(lowered(serde_json::json!({"source": "attr_keys", "equals": "x"})).is_err());
    assert!(lowered(serde_json::json!({"source": "span_name", "exists": true})).is_err());
    assert!(
        lowered(serde_json::json!({"source": "attr:k"})).is_err(),
        "a source asked nothing"
    );
    // A disjunct its own group already covers can never be why the group held.
    assert!(
        lowered(serde_json::json!({"any": [
            {"source": "attr:k", "exists": true},
            {"source": "attr:k", "equals": "v"},
        ]}))
        .is_err(),
        "`equals` beside `exists` of the same key is dead"
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

/// Every `where` answers exactly as the span predicate it replaced, over spans generated on its boundaries.
///
/// The migration's whole risk is that a translated condition means something slightly different, and the
/// difference shows only on an input nobody wrote a fixture for. So every retired declaration - detection's
/// `match` and `all_of`, a classification's `all_of`, a gate's `when`, `unless` and instrumentation scope, a span
/// fact's signal - frozen in `server/tests/message_goldens_parts/retired_span_predicates.json`, is asked by the
/// retired evaluator, and the clause's current `where` by the grammar, over spans built from that declaration's
/// own literals: each key present with the value, with its case flipped, inside other text, with another value,
/// empty and absent; each source alone and together; each declared scope and service name and neither. The
/// corpus half is `every_where_answers_as_the_predicate_it_replaced_over_the_corpus` in the goldens.
#[test]
fn every_where_answers_as_the_predicate_it_replaced() {
    use crate::rules::retired_span_predicates::{
        RetiredSubject, current_conditions, retired_holds,
    };
    use crate::rules::span_conditions::{SpanSubject, holds, lower};
    use std::collections::{BTreeSet, HashMap};

    let frozen: serde_json::Value = serde_json::from_slice(
        &std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/message_goldens_parts/retired_span_predicates.json"
        ))
        .expect("the frozen declarations are readable"),
    )
    .expect("the frozen declarations decode");
    let records = frozen["clauses"].as_array().expect("a list of clauses");
    let current = current_conditions(
        &crate::rules::assets::ParsedAssets::parse(&crate::rules::schema::embedded_sources())
            .expect("the embedded assets parse"),
    );
    assert!(
        records.len() > 200,
        "only {} frozen declarations",
        records.len()
    );

    // Every literal a record mentions, wherever in it, by what it is.
    fn literals(
        value: &serde_json::Value,
        keys: &mut BTreeSet<String>,
        values: &mut BTreeSet<String>,
        names: &mut BTreeSet<String>,
        scopes: &mut BTreeSet<String>,
        services: &mut BTreeSet<String>,
    ) {
        match value {
            serde_json::Value::Object(members) => {
                for (member, inner) in members {
                    let strings: Vec<String> = match inner {
                        serde_json::Value::String(text) => vec![text.clone()],
                        serde_json::Value::Array(items) => items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_string))
                            .collect(),
                        _ => Vec::new(),
                    };
                    match member.as_str() {
                        "attr_exists" | "attrs_present" => keys.extend(strings),
                        "attr_prefix" => {
                            for prefix in strings {
                                keys.insert(format!("{prefix}something"));
                                keys.insert(prefix);
                            }
                        }
                        "span_name" | "span_name_exact" => names.extend(strings),
                        "scope_name" | "name" | "one_of" => scopes.extend(strings),
                        "service_name" => services.extend(strings),
                        "key" => keys.extend(strings),
                        "value" | "needles" => values.extend(strings),
                        "sources" => keys.extend(
                            strings
                                .iter()
                                .filter_map(|source| source.strip_prefix("attr:"))
                                .map(str::to_string),
                        ),
                        _ => {}
                    }
                    literals(inner, keys, values, names, scopes, services);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    literals(item, keys, values, names, scopes, services);
                }
            }
            _ => {}
        }
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

    let mut compared = 0_usize;
    let mut disagreements: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for record in records {
        let clause = record["clause"].as_str().expect("a clause key");
        let Some((readable, condition)) = current.get(clause) else {
            missing.push(clause.to_string());
            continue;
        };
        let lowered = lower(condition, *readable)
            .unwrap_or_else(|e| panic!("{clause}: the current condition lowers: {e}"));
        let (mut keys, mut values, mut names, mut scopes, mut services) = Default::default();
        literals(
            record,
            &mut keys,
            &mut values,
            &mut names,
            &mut scopes,
            &mut services,
        );
        let keys: Vec<String> = keys.into_iter().collect();
        let mut values: Vec<String> = values.into_iter().collect();
        if values.is_empty() {
            values.push("a value".to_string());
        }
        values.push(String::new());
        let mut names: Vec<String> = names.into_iter().collect();
        names.extend(names.clone().iter().map(|n| format!("{n}.tail")));
        names.push("other.span".to_string());
        let mut scopes: Vec<Option<String>> = scopes
            .into_iter()
            .flat_map(|scope| [Some(format!("{scope}.extra")), Some(scope)])
            .collect();
        scopes.push(None);
        scopes.push(Some("other.scope".to_string()));
        let mut resources: Vec<HashMap<String, String>> = vec![HashMap::new()];
        for service in &services {
            resources.push(HashMap::from([(
                "service.name".to_string(),
                format!("my-{service}-app"),
            )]));
        }
        let mut attr_sets: Vec<HashMap<String, String>> = vec![HashMap::new()];
        for key in &keys {
            for value in &values {
                for variant in [value.clone(), flip(value), format!("prefix {value} suffix")] {
                    attr_sets.push(HashMap::from([(key.clone(), variant)]));
                }
            }
            attr_sets.push(HashMap::from([(key.clone(), "unrelated".to_string())]));
        }
        if keys.len() > 1 {
            for value in values.iter().take(2) {
                attr_sets.push(keys.iter().map(|k| (k.clone(), value.clone())).collect());
                for matching in &keys {
                    attr_sets.push(
                        keys.iter()
                            .map(|k| {
                                (
                                    k.clone(),
                                    if k == matching {
                                        value.clone()
                                    } else {
                                        "unrelated".to_string()
                                    },
                                )
                            })
                            .collect(),
                    );
                }
            }
        }
        for name in &names {
            for scope in &scopes {
                for resource in &resources {
                    for attrs in &attr_sets {
                        let old = retired_holds(
                            record,
                            &RetiredSubject {
                                span_name: name,
                                scope_name: scope.as_deref(),
                                attrs,
                                resource,
                            },
                        );
                        let new = holds(
                            &lowered,
                            &SpanSubject {
                                span_name: if readable.span_name { name } else { "" },
                                attrs,
                                scope_name: if readable.scope {
                                    scope.as_deref()
                                } else {
                                    None
                                },
                                resource: readable.resource.then_some(resource),
                                scope_version: None,
                                marks: 0,
                            },
                        );
                        compared += 1;
                        if old != new {
                            disagreements.push(format!(
                                "  {clause}: span `{name}` scope {scope:?} resource {resource:?} attrs {attrs:?} - \
                                 retired said {old}, `where` says {new}"
                            ));
                        }
                    }
                }
            }
        }
    }
    assert!(
        missing.is_empty(),
        "{} frozen declaration(s) have no current `where`: {missing:?}",
        missing.len()
    );
    assert!(compared > 5_000, "only {compared} comparisons");
    disagreements.sort();
    disagreements.dedup();
    assert!(
        disagreements.is_empty(),
        "{} of {compared} comparisons disagree:\n{}",
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
            "read": {
                "attribute": "shared"
            },
            "parse": "text",
            "where": {
                "source": "attr:left",
                "exists": true
            },
            "emit": "message"
        },
        {
            "id": "probe.z",
            "priority": 10,
            "read": {
                "attribute": "other"
            },
            "parse": "text",
            "where": {
                "source": "attr:right",
                "exists": true
            },
            "emit": "message"
        }
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
