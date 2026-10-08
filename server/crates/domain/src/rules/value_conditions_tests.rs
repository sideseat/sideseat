//! The value-predicate migration: every `where` on a value lowers exactly as the two-list set it replaced.

use crate::rules::schema::{PredicateSet, ValueCondition};

/// Every frozen retired set, found where its `where` now sits, lowers to the **same** evaluated expression and
/// meets the same compile-time analysis.
///
/// Equality of the lowered expression, not agreement on sample values: it is the evaluator's whole input, so two
/// equal expressions answer alike for every payload there is. A set that declared nothing placed no condition,
/// and its `where` is absent.
///
/// Two records left the frozen file after it was frozen, because their condition moved rather than changed:
/// openinference's `input_messages` entry filter and smolagents' `chat_message_reprs` where-item each excluded
/// the pseudo-roles `tool-call` and `tool-response`, and both became a `rendering` marker on the same reading,
/// so the turns are read and shown on the span that sent them instead of being dropped.
#[test]
fn every_value_where_lowers_as_the_set_it_replaced() {
    let frozen: serde_json::Value = serde_json::from_slice(
        &std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/message_goldens_parts/retired_value_predicates.json"
        ))
        .expect("the frozen sets are readable"),
    )
    .expect("the frozen sets decode");
    let sources = crate::rules::schema::embedded_sources();
    let mut compared = 0_usize;
    for record in frozen["sets"].as_array().expect("a list of sets") {
        let asset = record["asset"]
            .as_str()
            .expect("an asset path")
            .strip_prefix("assets/rules/")
            .expect("under the rules directory");
        let mut node: serde_json::Value = serde_json::from_slice(
            sources
                .get(asset)
                .unwrap_or_else(|| panic!("{asset} is embedded")),
        )
        .expect("the asset decodes");
        let path = record["path"].as_array().expect("a path");
        let (last, init) = path.split_last().expect("a path names a member");
        for step in init {
            node = match step {
                serde_json::Value::String(member) => node[member.as_str()].clone(),
                serde_json::Value::Number(index) => {
                    node[index.as_u64().expect("an index") as usize].clone()
                }
                serde_json::Value::Object(named) => node
                    .as_array()
                    .and_then(|items| items.iter().find(|item| item["id"] == named["id"]))
                    .cloned()
                    .unwrap_or_else(|| panic!("{asset}: no element {named:?} along {path:?}")),
                other => panic!("an unexpected step {other}"),
            };
        }
        let member = last.as_str().expect("a member name");
        let retired: PredicateSet =
            serde_json::from_value(record["retired"].clone()).expect("a retired set decodes");
        if record.get("dropped").is_some() {
            assert!(
                node.get(member).is_none(),
                "{asset} {path:?}: a set that declared nothing leaves no `{member}`"
            );
            assert!(retired.expression().is_none());
            continue;
        }
        let current: ValueCondition = serde_json::from_value(
            node.get(member)
                .unwrap_or_else(|| panic!("{asset} {path:?}: no `{member}`"))
                .clone(),
        )
        .unwrap_or_else(|e| panic!("{asset} {path:?}: the condition parses: {e}"));
        assert_eq!(
            current.expression(),
            retired.expression(),
            "{asset} {path:?}: the `where` lowers differently from the set it replaced"
        );
        assert_eq!(
            crate::rules::message_rules::predicate_defect(&current),
            crate::rules::message_rules::predicate_defect(&ValueCondition::from_set(&retired)),
            "{asset} {path:?}: the analysis answers differently"
        );
        compared += 1;
    }
    assert!(compared > 150, "only {compared} sets were compared");
}

/// A condition of any shape is a condition: groups nest, `not` negates, and the analysis still checks its atoms.
#[test]
fn a_value_where_of_any_shape_lowers_and_is_checked() {
    let parse = |value: serde_json::Value| -> ValueCondition {
        serde_json::from_value(value).expect("the condition parses")
    };
    let nested = parse(serde_json::json!({"any": [
        {"all": [{"path": "$.a", "exists": true}, {"path": "$.b", "exists": true}]},
        {"all": [{"path": "$.c", "exists": true}, {"path": "$.d", "exists": true}]},
    ]}));
    assert!(
        nested.set_view().is_none(),
        "two conjunctions in a disjunction are not a two-list set"
    );
    let holds = |condition: &ValueCondition, value: serde_json::Value| {
        crate::rules::message_rules::predicates_hold(&value, condition)
    };
    assert!(holds(&nested, serde_json::json!({"c": 1, "d": 2})));
    assert!(!holds(&nested, serde_json::json!({"a": 1, "d": 2})));
    let negated = parse(serde_json::json!({"not": {"path": "$.a", "exists": true}}));
    assert!(holds(&negated, serde_json::json!({})));
    assert!(!holds(&negated, serde_json::json!({"a": null})));
    // An atom that can never mean what it says is refused wherever it sits.
    let tautology = parse(serde_json::json!({"not": {"exists": true}}));
    assert!(crate::rules::message_rules::predicate_defect(&tautology).is_some());
}

/// A reading's `raw_where` asks what the two flags it replaced asked of the raw carrier text.
///
/// `require_non_empty` skipped the empty string and `require_non_blank` text that is blank once trimmed; the
/// shipped assets now say `{"non_empty": true}` and `{"non_blank": true}`, which must answer the same for every
/// shape of raw text, Unicode whitespace included.
#[test]
fn raw_where_asks_what_the_emptiness_flags_asked() {
    let texts = ["", " ", "\t\n", "\u{2003}", "x", " x ", "0", "null", "[]"];
    for (written, retired) in [
        (
            serde_json::json!({"non_empty": true}),
            (|t: &str| !t.is_empty()) as fn(&str) -> bool,
        ),
        (serde_json::json!({"non_blank": true}), |t: &str| {
            !t.trim().is_empty()
        }),
    ] {
        let condition: ValueCondition = serde_json::from_value(written.clone()).expect("parses");
        assert_eq!(
            crate::rules::message_rules::predicate_defect(&condition),
            None
        );
        for text in texts {
            assert_eq!(
                crate::rules::message_rules::predicates_hold(&serde_json::json!(text), &condition),
                retired(text),
                "{written} on {text:?}"
            );
        }
    }
    // Every shipped `raw_where` is one of the two.
    for (path, bytes) in crate::rules::schema::embedded_sources() {
        let asset: serde_json::Value = serde_json::from_slice(&bytes).expect("decodes");
        fn walk(value: &serde_json::Value, path: &str) {
            match value {
                serde_json::Value::Object(members) => {
                    if let Some(raw) = members.get("raw_where") {
                        assert!(
                            *raw == serde_json::json!({"non_empty": true})
                                || *raw == serde_json::json!({"non_blank": true}),
                            "{path}: a `raw_where` the migration did not write: {raw}"
                        );
                    }
                    members.values().for_each(|inner| walk(inner, path));
                }
                serde_json::Value::Array(items) => items.iter().for_each(|inner| walk(inner, path)),
                _ => {}
            }
        }
        walk(&asset, &path);
    }
}
