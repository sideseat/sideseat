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
    assert_eq!(
        plan.rule_count(),
        28,
        "the assets declare {} detection rules; the table they replaced had 28",
        plan.rule_count()
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
