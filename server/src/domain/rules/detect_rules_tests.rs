//! Gates on the detection slice: equivalence with the table it replaced, and refusal of a ruleset whose
//! order nobody owns.

use std::collections::HashMap;

use super::detect_rules::{DetectCompileError, DetectContext, compile};
use super::{ruleset, schema};

fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn the_detection_plan_holds_every_rule() {
    let plan = &ruleset().detect;
    // 28 producers, 29 compiled rules: one producer declares its **self-identification** as a separately ranked
    // alternative, because the predicates in one `match` are independently sufficient and its three signals
    // differ in strength - a defaulted `service.name` has to be ranked last, which put `gen_ai.system` there too.
    assert_eq!(
        plan.rule_count(),
        29,
        "the assets declare {} detection rules; the table they replaced had 28, plus one ranked alternative",
        plan.rule_count()
    );
    assert_eq!(
        plan.rules()
            .map(|rule| rule.label.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        28,
        "an alternative is further evidence for a label, not a label of its own"
    );
    assert_eq!(
        plan.slug_count(),
        26,
        "the assets declare {} SDK slugs; the table they replaced had 26",
        plan.slug_count()
    );
    for rule in plan.rules() {
        assert!(
            !rule.label.is_empty() && !rule.rule_id.is_empty() && !rule.rule_file.is_empty(),
            "every rule carries the label and the provenance the explain trace reports"
        );
        assert!(
            rule.doc.is_some(),
            "detection rule `{}` has no doc, and its rank is policy somebody must be able to review",
            rule.rule_id
        );
    }
}

#[test]
fn two_rules_at_one_rank_are_refused() {
    // Rank is explicit precisely because detection order is policy. Two rules sharing one would be
    // separated by load order, which is what the explicit rank exists to eliminate.
    let clash = br#"{
      "id": "t", "doc": "d",
      "detect": [
        {"id": "a", "doc": "d", "label": "A", "legacy_rank": 5, "match": {"attr_prefix": ["a."]}},
        {"id": "b", "doc": "d", "label": "B", "legacy_rank": 5, "match": {"attr_prefix": ["b."]}}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), clash.to_vec())]);
    assert!(matches!(
        compile(&sources),
        Err(DetectCompileError::DuplicateRank { .. })
    ));
}

#[test]
fn a_rule_with_no_signal_is_refused() {
    let bare = br#"{
      "id": "t", "doc": "d",
      "detect": [{"id": "a", "doc": "d", "label": "A", "legacy_rank": 1, "match": {}}]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), bare.to_vec())]);
    assert!(matches!(
        compile(&sources),
        Err(DetectCompileError::NoSignal { .. })
    ));
}

#[test]
fn a_slug_claimed_twice_is_refused() {
    // A declaration naming it would have two answers, and two answers is not an answer.
    let dup = br#"{
      "id": "t", "doc": "d",
      "sdk_slugs": [{"slug": "x", "label": "A"}, {"slug": "x", "label": "B"}]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), dup.to_vec())]);
    assert!(matches!(
        compile(&sources),
        Err(DetectCompileError::DuplicateSlug { .. })
    ));
}

#[test]
fn a_text_source_must_name_something_the_engine_can_read() {
    let bad = br#"{
      "id": "t", "doc": "d",
      "detect": [{"id": "a", "doc": "d", "label": "A", "legacy_rank": 1,
                  "match": {"text_contains": {"sources": ["whatever"], "needles": ["x"]}}}]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), bad.to_vec())]);
    assert!(matches!(
        compile(&sources),
        Err(DetectCompileError::BadTextSource { .. })
    ));
}

#[test]
fn rank_decides_which_of_two_matching_rules_wins() {
    let ordered = br#"{
      "id": "t", "doc": "d",
      "detect": [
        {"id": "broad", "doc": "d", "label": "Broad", "legacy_rank": 90, "match": {"attr_prefix": ["x."]}},
        {"id": "narrow", "doc": "d", "label": "Narrow", "legacy_rank": 10, "match": {"attr_prefix": ["x.y."]}}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), ordered.to_vec())]);
    let plan = compile(&sources).expect("distinct ranks compile");
    let span_attrs = attrs(&[("x.y.z", "1")]);
    let resource_attrs = attrs(&[]);
    let hit = plan
        .resolve(&DetectContext {
            span_name: "s",
            span_attrs: &span_attrs,
            resource_attrs: &resource_attrs,
        })
        .expect("both rules match this span");
    assert_eq!(
        hit.label, "Narrow",
        "the lower rank wins, and it is declared rather than implied by position in a file"
    );
}

#[test]
fn detection_asks_no_question_about_framework_identity() {
    // The circularity guard: detection *produces* the label, so nothing it examines may be one.
    let source = include_str!("detect_rules.rs");
    let decl = source
        .split("pub struct DetectContext")
        .nth(1)
        .and_then(|s| s.split('}').next())
        .expect("the detection context is declared here");
    for forbidden in ["framework", "label"] {
        assert!(
            !decl.contains(forbidden),
            "`DetectContext` carries `{forbidden}`: detection cannot consume the thing it produces"
        );
    }
    // And the assets really do carry the vocabulary, so the engine being clean is not vacuous.
    let all: String = schema::embedded_sources()
        .values()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect();
    for signal in [
        "crewAI-telemetry",
        "langgraph.",
        "telemetry.sdk.name",
        "claude_code.",
        "strands agent",
        "microsoft.agent_framework",
    ] {
        assert!(
            all.contains(signal),
            "detection signal `{signal}` is declared in no asset, so nothing declares it at all"
        );
    }
}

/// A first-present phrase search over mixed sources is refused on **both** compile paths.
///
/// Compilation splits sources into "the span name" and a list of attribute keys, so the declared order between
/// the two is lost and the name is always reached first. The detection path validates rules of its own and does
/// not go through `gate_defect`, so this was refused for a field source and accepted here - one definition now,
/// because that is the shape of the defect.
#[test]
fn a_mixed_first_present_search_is_refused_here_too() {
    let mixed = br#"{
      "id": "t", "doc": "d",
      "detect": [
        {"id": "x", "doc": "d", "label": "X", "legacy_rank": 10,
         "match": {"text_contains": {"sources": ["attr:model", "span_name"], "needles": ["embed"],
                                     "first_present_source": true}}}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), mixed.to_vec())]);
    assert!(
        compile(&sources).is_err(),
        "the declared order between the span name and an attribute is not preserved, so this must not compile"
    );

    // One kind of source is fine, and so is the same mix *without* the flag - where every source is searched and
    // there is no order to lose.
    for asset in [
        br#"{"id":"t","doc":"d","detect":[
            {"id":"x","doc":"d","label":"X","legacy_rank":10,
             "match":{"text_contains":{"sources":["attr:a","attr:b"],"needles":["e"],"first_present_source":true}}}]}"#
            .to_vec(),
        br#"{"id":"t","doc":"d","detect":[
            {"id":"x","doc":"d","label":"X","legacy_rank":10,
             "match":{"text_contains":{"sources":["span_name"],"needles":["e"],"first_present_source":true}}}]}"#
            .to_vec(),
        br#"{"id":"t","doc":"d","detect":[
            {"id":"x","doc":"d","label":"X","legacy_rank":10,
             "match":{"text_contains":{"sources":["attr:model","span_name"],"needles":["e"]}}}]}"#
            .to_vec(),
    ] {
        let sources = std::collections::BTreeMap::from([("t.json".to_string(), asset)]);
        assert!(
            compile(&sources).is_ok(),
            "there is no order to lose here: {:?}",
            compile(&sources).err()
        );
    }
}

/// A literal another in the same list already covers is refused: it can never be why a rule matched.
///
/// Two shipped declarations were exactly that. `span_name: ["LangGraph", "LangGraph."]` - a prefix subsumes its
/// own extension, so the separator form was dead and the bare name additionally claimed every unrelated span
/// merely starting with those letters. And `span_attr_contains` holding `"langgraph_` beside `langgraph_`, where
/// the broader substring always fires first. Both read as precision the rule did not have.
///
/// Also refused per dimension kind, because "covers" differs: a prefix list, a substring list, and an exact list
/// where only a duplicate covers.
#[test]
fn a_literal_another_already_covers_is_refused() {
    let compiled = |detect: &str| {
        let asset = format!(r#"{{"id":"t","doc":"d","detect":[{detect}]}}"#);
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            asset.into_bytes(),
        )]))
    };
    let subsumed = |detect: &str| {
        matches!(
            compiled(detect),
            Err(DetectCompileError::SubsumedLiteral { .. })
        )
    };

    // Prefix: the extension is dead beside the bare form. The shape the shipped asset had.
    assert!(subsumed(
        r#"{"id":"a","doc":"d","label":"A","legacy_rank":1,"match":{"span_name":["LangGraph","LangGraph."]}}"#
    ));
    assert!(subsumed(
        r#"{"id":"a","doc":"d","label":"A","legacy_rank":1,"match":{"attr_prefix":["ai.","ai.telemetry."]}}"#
    ));
    // Substring, per key: the quoted form is dead beside the bare one. The other shipped shape.
    assert!(subsumed(
        r#"{"id":"a","doc":"d","label":"A","legacy_rank":1,"match":{"span_attr_contains":[
             {"key":"metadata","value":"langgraph_"},{"key":"metadata","value":"\"langgraph_"}]}}"#
    ));
    // Substring under two *different* keys says nothing: they are not in one another's list.
    assert!(
        compiled(
            r#"{"id":"a","doc":"d","label":"A","legacy_rank":1,"match":{"span_attr_contains":[
                 {"key":"one","value":"langgraph_"},{"key":"two","value":"\"langgraph_"}]}}"#
        )
        .is_ok(),
        "two substrings of different attributes do not cover each other"
    );
    // Exact: only a duplicate covers, and `LangGraph.` is a perfectly good separate exact name.
    assert!(subsumed(
        r#"{"id":"a","doc":"d","label":"A","legacy_rank":1,"match":{"span_name_exact":["LangGraph","LangGraph"]}}"#
    ));
    assert!(
        compiled(
            r#"{"id":"a","doc":"d","label":"A","legacy_rank":1,"match":{"span_name_exact":["LangGraph","LangGraph."]}}"#
        )
        .is_ok(),
        "an exact list is covered only by a duplicate - a prefix relation between two exact names is not one"
    );
    // And an exact name beside a prefix in the *other* dimension is the whole point of splitting them.
    assert!(
        compiled(
            r#"{"id":"a","doc":"d","label":"A","legacy_rank":1,"match":{"span_name_exact":["LangGraph"],"span_name":["LangGraph."]}}"#
        )
        .is_ok(),
        "exactly `LangGraph` beside the `LangGraph.` prefix is two statements, which is what the split is for"
    );
}

/// A span name is matched by prefix or exactly, and the two say different things.
///
/// As one "equals or starts with" dimension the bare name claimed every span merely starting with those letters,
/// which is what a producer with a similarly-named span would have been attributed to.
#[test]
fn an_exact_span_name_does_not_claim_names_that_merely_start_with_it() {
    let plan = &ruleset().detect;
    let detected = |name: &str| {
        plan.resolve(&DetectContext {
            span_name: name,
            span_attrs: &attrs(&[]),
            resource_attrs: &attrs(&[]),
        })
        .map(|found| found.label.to_string())
    };

    assert_eq!(detected("LangGraph").as_deref(), Some("LangGraph"));
    assert_eq!(detected("LangGraph.step").as_deref(), Some("LangGraph"));
    assert_ne!(
        detected("LangGraphicalTask").as_deref(),
        Some("LangGraph"),
        "a span whose name merely starts with those letters is not this producer's"
    );
}

/// A producer naming **itself** outranks a convention namespace, because they are separately ranked.
///
/// The predicates in one `match` are independently sufficient, so one rank has to be placed for the *weakest* of
/// them. Strands' weakest is a `service.name` the SideSeat SDK defaults, which must be ranked last or it claims
/// every framework using the SDK - and that put `gen_ai.system: "strands-agents"`, the strongest evidence there
/// is, behind `openinference.`. A Strands span carrying any OpenInference attribute was labelled OpenInference.
///
/// Not reachable from the captured corpus: only one `_synthetic` span carries an `openinference.*` attribute and
/// none carries both, so this is a shape the format could not express rather than a measured mislabelling.
#[test]
fn a_producer_naming_itself_outranks_a_convention_namespace() {
    let plan = &ruleset().detect;
    let label = |span: &str, pairs: &[(&str, &str)], resource: &[(&str, &str)]| {
        plan.resolve(&DetectContext {
            span_name: span,
            span_attrs: &attrs(pairs),
            resource_attrs: &attrs(resource),
        })
        .map(|found| found.label.to_string())
    };

    assert_eq!(
        label("chat", &[("gen_ai.system", "strands-agents")], &[]).as_deref(),
        Some("StrandsAgents")
    );
    assert_eq!(
        label(
            "chat",
            &[
                ("gen_ai.system", "strands-agents"),
                ("openinference.span.kind", "LLM"),
            ],
            &[],
        )
        .as_deref(),
        Some("StrandsAgents"),
        "the producer named itself; a convention namespace says which conventions it used, not who it is"
    );
    // The convention alone still answers, or the alternative would have taken its rule's place rather than
    // sitting beside it.
    assert_eq!(
        label("chat", &[("openinference.span.kind", "LLM")], &[]).as_deref(),
        Some("OpenInference")
    );
    // And the weak signal stays where it has to be: it is still ranked behind every framework that uses the SDK,
    // which is the whole reason the two could not share a rank.
    assert_eq!(
        label(
            "chat",
            &[("openinference.span.kind", "LLM")],
            &[("service.name", "strands-agents")],
        )
        .as_deref(),
        Some("OpenInference"),
        "a defaulted service name must not outrank a convention the span actually carries"
    );
}

/// A `service.name` is matched by substring, deliberately, and the equality arm beside it was dead.
///
/// The breadth is intended: a user names their own service and the SDK's name sits inside it. Narrowing this to
/// equality was tried - the captured corpus cannot see the difference, since every `service.name` in it that
/// matches a declared literal matches exactly - and two unit tests are the evidence that it is wrong.
#[test]
fn a_service_name_identifies_a_producer_by_substring() {
    let plan = &ruleset().detect;
    let label = |service: &str| {
        plan.resolve(&DetectContext {
            span_name: "chat",
            span_attrs: &attrs(&[]),
            resource_attrs: &attrs(&[("service.name", service)]),
        })
        .map(|found| found.label.to_string())
    };
    assert_eq!(label("openai-agents").as_deref(), Some("OpenAIAgents"));
    assert_eq!(
        label("my-app-openai-agents-v1").as_deref(),
        Some("OpenAIAgents"),
        "a user's own service name holding the SDK's identifies it"
    );
    assert_eq!(label("my-app").as_deref(), None);
}

/// A rule an earlier rule always satisfies first is refused: its answer can never be reached.
///
/// The same defect as a subsumed literal, one level up - and for a *detection* rule it means silently never
/// attributing its producer at all. Sound rather than complete, deliberately: a false refusal breaks a build for a
/// reason nobody can act on, so anything the implication relation does not recognise answers "no shadow proven".
#[test]
fn a_rule_an_earlier_one_always_satisfies_is_refused() {
    let compiled = |detect: serde_json::Value| {
        let asset = serde_json::json!({"id": "t", "doc": "d", "detect": detect});
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            serde_json::to_vec(&asset).expect("serialises"),
        )]))
    };
    let shadowed = |detect: serde_json::Value| {
        matches!(
            compiled(detect),
            Err(DetectCompileError::ShadowedRule { .. })
        )
    };
    let rule = |id: &str, rank: i32, spec: serde_json::Value| serde_json::json!({"id": id, "doc": "d", "label": id, "legacy_rank": rank, "match": spec});

    // A key's existence covers any statement about that key's value.
    assert!(shadowed(serde_json::json!([
        rule("a", 10, serde_json::json!({"attr_exists": ["k"]})),
        rule(
            "b",
            20,
            serde_json::json!({"attr_equals": [{"key": "k", "value": "v"}]})
        ),
    ])));
    // A shorter attribute prefix covers a longer one.
    assert!(shadowed(serde_json::json!([
        rule("a", 10, serde_json::json!({"attr_prefix": ["ai."]})),
        rule(
            "b",
            20,
            serde_json::json!({"attr_prefix": ["ai.telemetry."]})
        ),
    ])));
    // A span-name prefix covers an exact name under it.
    assert!(shadowed(serde_json::json!([
        rule("a", 10, serde_json::json!({"span_name": ["Graph."]})),
        rule(
            "b",
            20,
            serde_json::json!({"span_name_exact": ["Graph.step"]})
        ),
    ])));
    // A case-insensitive equality covers the case-sensitive one.
    assert!(shadowed(serde_json::json!([
        rule(
            "a",
            10,
            serde_json::json!({"attr_equals_ignore_case": [{"key": "k", "value": "LLM"}]})
        ),
        rule(
            "b",
            20,
            serde_json::json!({"attr_equals": [{"key": "k", "value": "llm"}]})
        ),
    ])));

    // **Not** shadowed, and each of these is a shape the refusal must not reject.
    for (what, detect) in [
        (
            "the other direction: a broad rule ranked *after* a narrow one is reachable",
            serde_json::json!([
                rule(
                    "a",
                    10,
                    serde_json::json!({"attr_prefix": ["ai.telemetry."]})
                ),
                rule("b", 20, serde_json::json!({"attr_prefix": ["ai."]})),
            ]),
        ),
        (
            "different keys say nothing about each other",
            serde_json::json!([
                rule("a", 10, serde_json::json!({"attr_exists": ["one"]})),
                rule(
                    "b",
                    20,
                    serde_json::json!({"attr_equals": [{"key": "two", "value": "v"}]})
                ),
            ]),
        ),
        (
            "a later rule needing *more* than the earlier one is reachable only if some conjunct is covered - \
             here none is",
            serde_json::json!([
                rule("a", 10, serde_json::json!({"attr_exists": ["one"]})),
                serde_json::json!({"id": "b", "doc": "d", "label": "b", "legacy_rank": 20,
                    "match": {"attr_exists": ["two"]}}),
            ]),
        ),
        (
            "a phrase search is not analysed on either side, so nothing is proven about it",
            serde_json::json!([
                rule(
                    "a",
                    10,
                    serde_json::json!({"text_contains": {"sources": ["span_name"], "needles": ["x"]}})
                ),
                rule("b", 20, serde_json::json!({"attr_exists": ["k"]})),
            ]),
        ),
        (
            "a rule that **supersedes** its shadower is reachable: `supersedes` orders ahead of rank, so the \
             broader rule loses to it - which is the shape `supersedes` exists for",
            serde_json::json!([
                rule("a", 10, serde_json::json!({"attr_prefix": ["ai."]})),
                serde_json::json!({"id": "b", "doc": "d", "label": "b", "legacy_rank": 20,
                    "match": {"attr_prefix": ["ai.telemetry."]}, "supersedes": ["a"]}),
            ]),
        ),
    ] {
        assert!(compiled(detect).is_ok(), "wrongly refused: {what}");
    }
}

/// A value outside what its quantity can hold is **malformed**, not a measurement.
///
/// Nothing checked, so `-5` parsed as an `i64` and became a real token count: it summed into the trace total,
/// priced at a negative cost, and could cancel a genuine counter. A negative count, duration, limit or status is
/// not a small measurement - it is not a measurement. `DefectKind::OutOfRange` had existed in the outcome algebra
/// since cycle 10 and nothing produced it.
///
/// Present-and-unusable is what `Malformed` already means, so the chain's own `on_malformed` policy decides what
/// follows: a chain that steps over unreadable values reaches the next spelling, which is the behaviour asserted
/// below.
#[test]
fn a_value_outside_what_a_quantity_can_hold_is_malformed() {
    use crate::domain::rules::schema::FieldTarget;
    use crate::domain::rules::span_fields::Reading;

    let resolve = |asset: &str, pairs: &[(&str, &str)]| {
        let plan =
            crate::domain::rules::span_fields::compile(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                asset.as_bytes().to_vec(),
            )]))
            .expect("the probe compiles");
        plan.resolve("chat", &attrs(pairs), &[])
    };

    // A negative counter: present, and not a count.
    let one = r#"{"id":"t","doc":"d","span_fields":[
        {"id":"f","doc":"d","target":"usage_input_tokens",
         "sources":[{"id":"probe.only","attribute":"tokens"}]}]}"#;
    let resolved = resolve(one, &[("tokens", "-5")]);
    let found = resolved
        .iter()
        .find(|r| r.target == FieldTarget::UsageInputTokens)
        .expect("the rule resolved");
    assert_eq!(
        found.reading,
        Reading::Absent,
        "the only source was refused, so the field has no answer - not a negative one"
    );
    // And the refusal names it, so the value is reported rather than silently dropped: an out-of-range reading is
    // present-and-unusable, which is a different fact from nobody having written it.
    assert!(
        found.refused.iter().any(|refusal| matches!(
            &refusal.reading,
            Reading::Malformed { detail } if detail.contains("outside what this field can hold")
        )),
        "the out-of-range value is not reported: {:?}",
        found.refused
    );
    // A positive one is untouched, or this is a ban on counters rather than a bound.
    let resolved = resolve(one, &[("tokens", "7")]);
    assert_eq!(
        resolved
            .iter()
            .find(|r| r.target == FieldTarget::UsageInputTokens)
            .map(|r| r.reading.clone()),
        Some(Reading::Integer(7))
    );

    // **The source's own `on_malformed` policy governs**, exactly as it does for a value of the wrong type - no
    // new mechanism, because the argument is the same one that policy already carries: a wrong value in a
    // producer's own usage attribute means answering from a *second* key reports another framework's counter as
    // this call's. So the default stops, and a source that says the next spelling is the same producer's
    // continues.
    let stopping = r#"{"id":"t","doc":"d","span_fields":[
        {"id":"f","doc":"d","target":"usage_input_tokens",
         "sources":[{"id":"probe.bad","attribute":"first"},{"id":"probe.good","attribute":"second"}]}]}"#;
    let continuing = r#"{"id":"t","doc":"d","span_fields":[
        {"id":"f","doc":"d","target":"usage_input_tokens",
         "sources":[{"id":"probe.bad","attribute":"first","on_malformed":"continue"},
                    {"id":"probe.good","attribute":"second"}]}]}"#;
    let answer = |asset: &str| {
        resolve(asset, &[("first", "-1"), ("second", "12")])
            .iter()
            .find(|r| r.target == FieldTarget::UsageInputTokens)
            .map(|r| (r.reading.clone(), r.refused.len()))
            .expect("resolved")
    };
    assert_eq!(
        answer(stopping),
        (Reading::Absent, 1),
        "the default ends the chain and reports the source that ended it"
    );
    assert_eq!(
        answer(continuing),
        (Reading::Integer(12), 1),
        "a source declared `continue` steps over it, and the refusal is still reported"
    );

    // A probability is bounded on both sides; a penalty is legitimately negative and is not bounded at all.
    let bounded = r#"{"id":"t","doc":"d","span_fields":[
        {"id":"p","doc":"d","target":"gen_ai_top_p","sources":[{"id":"probe.p","attribute":"p"}]},
        {"id":"q","doc":"d","target":"gen_ai_frequency_penalty","sources":[{"id":"probe.q","attribute":"q"}]}]}"#;
    let resolved = resolve(bounded, &[("p", "1.5"), ("q", "-1.5")]);
    let found = |target: FieldTarget| {
        resolved
            .iter()
            .find(|r| r.target == target)
            .map(|r| (r.reading.clone(), r.refused.len()))
            .expect("resolved")
    };
    assert_eq!(
        found(FieldTarget::GenAiTopP),
        (Reading::Absent, 1),
        "a probability above 1 is not a probability, so the field has no answer and the value is reported"
    );
    assert_eq!(
        found(FieldTarget::GenAiFrequencyPenalty),
        (Reading::Float(-1.5), 0),
        "a penalty is legitimately negative, so bounding it would refuse a producer's honest value"
    );
}

/// "Nothing recognised this producer" carries evidence: the rules that read a key the span has.
///
/// It was a bare `None` everywhere, at exactly the moment an operator needs to know why - and no explain trace
/// exists, though several `doc` fields are collected "for" one. A full trace over every rule would bury the answer;
/// what identifies the common failure is narrow: a producer writes the attribute a rule reads, with a value the
/// rule does not declare. The remedy is then to declare the value, which the report names.
#[test]
fn nothing_recognised_this_producer_says_which_rules_were_close() {
    let plan = &ruleset().detect;
    let near = |span: &str, pairs: &[(&str, &str)], resource: &[(&str, &str)]| {
        plan.near_misses(&DetectContext {
            span_name: span,
            span_attrs: &attrs(pairs),
            resource_attrs: &attrs(resource),
        })
    };

    // An unrecognised producer conforming to the conventions: it writes `gen_ai.system`, with a value no rule
    // declares.
    let found = near("chat", &[("gen_ai.system", "acme-agents")], &[]);
    assert!(
        found
            .iter()
            .any(|miss| miss.carrier == "gen_ai.system" && miss.found == "acme-agents"),
        "an unknown value in a key rules read must be reported: {found:?}"
    );
    assert!(
        found
            .iter()
            .all(|miss| !miss.expected.is_empty() && !miss.rule_id.is_empty()),
        "each near miss names the declaration and what it required, or it is not actionable"
    );

    // A service name that merely resembles one, which is the other common shape.
    let found = near("chat", &[], &[("service.name", "my-own-app")]);
    assert!(
        found
            .iter()
            .any(|miss| miss.carrier == "service.name" && miss.found == "my-own-app")
    );

    // **Silence where there is nothing to say.** A span carrying no key any rule reads produces no report:
    // listing every rule is what makes an explanation useless.
    assert!(
        near("chat", &[("acme.private", "1")], &[]).is_empty(),
        "a span with no key any rule reads is not a near miss"
    );

    // And a span that **is** attributed reports nothing at all. Asked inside the function rather than left to
    // its caller: without it, a correctly attributed Strands span reported six other rules disagreeing about
    // `gen_ai.system`, so the report's usefulness depended on where it was called from.
    assert!(
        near("chat", &[("gen_ai.system", "strands-agents")], &[]).is_empty(),
        "a span something recognised has nothing to explain"
    );
}

/// **What the format can express for a producer nobody has captured, measured rather than asserted.**
///
/// The series' central claim is that adding a framework is an *asset edit* and adding a primitive is a code
/// change. Cycle 25 put four hypothetical producers to it; two were said to be expressible. Running them proved
/// one of those two wrong, which is the whole reason for running them.
///
/// **Expressible, proven below:** a conversation written as a flat, numerically indexed attribute family -
/// `chat.0.content`, `chat.1.content`, … - with no JSON anywhere and an unbounded index. `chat.97` is read exactly
/// like `chat.0`, because the format declares no bound.
///
/// **Not expressible, also proven below:** a message whose **role comes from a sibling member** under the
/// producer's own name - `kind: "in"` means a user turn. `wrap` beside `indexed_family` is *refused* ("an indexed
/// family assembles each entry itself"), so `role_from` and `role_map` cannot be reached there: the family emits
/// `{content, kind}`, nothing states that `kind` holds the role, and normalisation defaults the turn to `user` -
/// so this producer's assistant replies would be reported as the user's. The missing primitive is exactly that: an
/// indexed family that can name which assembled member is the role, and map that member's vocabulary.
///
/// Two further shapes are **not** expressible and are recorded rather than built, per the standing ruling that a
/// primitive waits for a real producer: a base64 protobuf blob holding request and response (needs a
/// descriptor-driven decode, which brings schema distribution and a parser dependency), and token usage reported
/// only as a per-event *delta* (needs `reduce: sum` over event occurrences, where cumulative-versus-delta
/// semantics have to be evidenced before they can be assumed).
#[test]
fn what_an_unseen_producer_can_and_cannot_declare() {
    use crate::domain::rules::MessageContext;
    use crate::domain::rules::message_rules::{MessageCompileError, compile};

    let compiled = |messages: &str| {
        let asset = format!(
            r#"{{"id":"acme-probe","doc":"A producer nobody has captured.","messages":{messages}}}"#
        );
        compile(&std::collections::BTreeMap::from([(
            "acme-probe.json".to_string(),
            asset.into_bytes(),
        )]))
    };

    // Expressible: the flat indexed family, declared entirely in an asset.
    let plan = compiled(
        r#"[{"id":"acme.flat_family","doc":"d","read":{"indexed_family":"chat"},
             "parse":"text","emit":"message","legacy_rank":1}]"#,
    )
    .expect("a flat indexed family is an asset edit");

    let span_attrs = attrs(&[
        ("chat.0.kind", "in"),
        ("chat.0.content", "what is the weather"),
        ("chat.1.kind", "out"),
        ("chat.1.content", "it is raining"),
        // An index far past any bound, because the format declares none.
        ("chat.97.kind", "in"),
        ("chat.97.content", "and tomorrow"),
    ]);
    let ctx = MessageContext::for_span("acme.turn", &span_attrs, false);
    let mut carried: Vec<(String, String)> = plan
        .run(&ctx)
        .iter()
        .filter_map(|emission| {
            Some((
                emission.carrier.name().to_string(),
                emission.value.get("content")?.as_str()?.to_string(),
            ))
        })
        .collect();
    carried.sort();
    assert_eq!(
        carried,
        vec![
            ("chat.0".to_string(), "what is the weather".to_string()),
            ("chat.1".to_string(), "it is raining".to_string()),
            ("chat.97".to_string(), "and tomorrow".to_string()),
        ],
        "three turns, each tagged with its own index - and `chat.97` is read like `chat.0`, since no bound is \
         declared"
    );

    // **Not** expressible: the role in a sibling member. Refused rather than silently ignored, which is the
    // format behaving correctly - and the limit is that there is nothing else to declare instead.
    let refused = compiled(
        r#"[{"id":"acme.role_from_sibling","doc":"d","read":{"indexed_family":"chat"},
             "parse":"text","emit":"message","legacy_rank":1,
             "wrap":{"content_from_any_of":["$.content"],"role_from":"$.kind",
                     "role_map":{"in":"user","out":"assistant"}}}]"#,
    )
    .expect_err("an indexed family cannot state which member holds the role");
    assert!(
        matches!(refused, MessageCompileError::Inexpressible { .. }),
        "the refusal should say the declaration cannot be executed: {refused}"
    );

    // And what that costs: the family emits the producer's own member names, so nothing downstream can tell an
    // assistant turn from a user one.
    let emitted: Vec<String> = plan
        .run(&ctx)
        .iter()
        .map(|emission| emission.value.to_string())
        .collect();
    assert!(
        emitted.iter().all(|value| !value.contains("\"role\"")),
        "no emission carries a role, which is the limit this case measures: {emitted:?}"
    );
}
