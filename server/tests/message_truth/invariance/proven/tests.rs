//! The proven gaps set aside exactly what they prove, and nothing else.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::json;

use super::super::open_order::reconstruction;
use super::*;

type Side = (Recon, BTreeMap<String, String>, Truth);

fn side(label: &str) -> Side {
    let fixtures: BTreeMap<String, Vec<PathBuf>> = crate::discover_fixtures().into_iter().collect();
    let (recon, labels) = reconstruction(label, &fixtures[label]);
    let truth = super::super::super::Truths::load()
        .of(label)
        .expect("a truth describes the fixture");
    (recon, labels, truth)
}

fn holds(release: &Side, current: &Side) -> bool {
    explains(
        (&release.0, &release.1, &release.2),
        (&current.0, &current.1, &current.2),
    )
}

/// The first block of a view of `kind` showing `fact`, by the release's truth.
fn block_of(side: &mut Side, kind: ViewKind, fact: impl Fn(&Fact) -> bool) -> Option<&mut Block> {
    let (recon, _, truth) = side;
    let wanted: Vec<Fact> = truth.facts.iter().filter(|f| fact(f)).cloned().collect();
    recon
        .views
        .iter_mut()
        .filter(|v| v.kind == kind)
        .flat_map(|v| v.blocks.iter_mut())
        .find(|b| wanted.iter().any(|f| shows(f, b, None) != Shows::No))
}

/// Removes every block showing `fact` from the trace and session views.
fn drop_everywhere(side: &mut Side, fact: impl Fn(&Fact) -> bool) -> bool {
    let (recon, _, truth) = side;
    let wanted: Vec<Fact> = truth.facts.iter().filter(|f| fact(f)).cloned().collect();
    let mut dropped = false;
    for view in recon
        .views
        .iter_mut()
        .filter(|v| matches!(v.kind, ViewKind::Trace | ViewKind::Session | ViewKind::Feed))
    {
        let before = view.blocks.len();
        view.blocks
            .retain(|b| !wanted.iter().any(|f| shows(f, b, None) != Shows::No));
        dropped |= view.blocks.len() != before;
    }
    dropped
}

/// The positive control: each release differs from the current one exactly where its proven gaps say - results it
/// does not export (ag2 1.0.2), calls read from another span in an order no payload holds (ag2 1.0.4), calls without
/// their ids in a run recorded on one span (the google-genai beta).
#[test]
fn a_release_differing_only_where_its_gaps_prove_is_the_same_conversation() {
    for (release, current) in [
        ("ag2/native@1.0.2/tool_use", "ag2/native/tool_use"),
        ("ag2/native@1.0.4/tool_use", "ag2/native/tool_use"),
        ("ag2/native@1.0.4/streaming", "ag2/native/streaming"),
        (
            "google-genai/native@1.0b1/streaming",
            "google-genai/native/streaming",
        ),
    ] {
        assert!(
            holds(&side(release), &side(current)),
            "{release} against {current}"
        );
    }
}

/// M1: a difference no gap proves still fails - the prompt, which every release exports, gone.
#[test]
fn an_unproven_difference_still_fails() {
    let current = side("ag2/native/tool_use");
    let mut release = side("ag2/native@1.0.2/tool_use");
    assert!(drop_everywhere(&mut release, |f| f.kind == "user_text"));
    assert!(!holds(&release, &current));
}

/// M2: a proof for another release excuses nothing here - 1.0.4 exports the results 1.0.2 and 1.0.3 do not.
#[test]
fn a_gap_proven_for_another_release_excuses_nothing() {
    let current = side("ag2/native/tool_use");
    let mut release = side("ag2/native@1.0.4/tool_use");
    assert!(drop_everywhere(&mut release, |f| f.kind == "tool_result"));
    assert!(!holds(&release, &current));
}

/// M3: a call compared without its id is still compared on its name and arguments.
#[test]
fn a_call_without_its_id_is_held_to_its_name_and_arguments() {
    let current = side("google-genai/native/streaming");
    for edit in [
        (|b: &mut Block| b.content["name"] = json!("get_forecast")) as fn(&mut Block),
        |b: &mut Block| b.content["input"]["city"] = json!("Paris"),
    ] {
        let mut release = side("google-genai/native@1.0b1/streaming");
        let block =
            block_of(&mut release, ViewKind::Trace, |f| f.kind == "tool_call").expect("the call");
        edit(block);
        block.refresh();
        assert!(!holds(&release, &current));
    }
}

/// M4: a part read from another span may move in the feed only within its response, and keeps its content.
#[test]
fn a_part_placed_by_another_span_keeps_its_content_and_its_response() {
    let current = side("ag2/native/streaming");
    let mut release = side("ag2/native@1.0.4/streaming");
    let block =
        block_of(&mut release, ViewKind::Feed, |f| f.kind == "tool_call").expect("the call");
    block.content["input"]["city"] = json!("Paris");
    block.refresh();
    assert!(!holds(&release, &current));

    let mut release = side("ag2/native@1.0.4/streaming");
    let truth = release.2.clone();
    let feed = release
        .0
        .views
        .iter_mut()
        .find(|v| v.kind == ViewKind::Feed)
        .expect("the feed");
    let at = feed
        .blocks
        .iter()
        .position(|b| {
            truth
                .facts
                .iter()
                .any(|f| f.kind == "tool_call" && shows(f, b, None) != Shows::No)
        })
        .expect("the call in the feed");
    // Ahead of the answer: the newest response's, not its own.
    let moved = feed.blocks.remove(at);
    feed.blocks.insert(0, moved);
    assert!(!holds(&release, &current));
}

/// M5: a block no gap places elsewhere keeps its place in the feed.
#[test]
fn a_block_no_gap_places_elsewhere_keeps_its_place() {
    let current = side("ag2/native/streaming");
    let mut release = side("ag2/native@1.0.4/streaming");
    let feed = release
        .0
        .views
        .iter_mut()
        .find(|v| v.kind == ViewKind::Feed)
        .expect("the feed");
    feed.blocks.swap(0, 1);
    assert!(!holds(&release, &current));
}

/// Parallel calls whose order no payload holds are compared as a set, and each still by its content.
#[test]
fn an_unordered_call_is_still_held_to_its_content() {
    let current = side("ag2/native/tool_use");
    let mut release = side("ag2/native@1.0.4/tool_use");
    let block = block_of(&mut release, ViewKind::Trace, |f| f.kind == "tool_call").expect("a call");
    block.content["input"]["city"] = json!("Atlantis");
    block.refresh();
    assert!(!holds(&release, &current));
}

/// A part placed by another span stays within its response even where that response's answer is unasserted: an
/// unasserted model output still bounds a response.
#[test]
fn an_unasserted_answer_still_bounds_a_response() {
    let current = side("google-genai/native/streaming");
    let mut release = side("google-genai/native@1.0b1/streaming");
    let truth = release.2.clone();
    let feed = release
        .0
        .views
        .iter_mut()
        .find(|v| v.kind == ViewKind::Feed)
        .expect("the feed");
    let at = feed
        .blocks
        .iter()
        .position(|b| {
            truth
                .facts
                .iter()
                .any(|f| f.kind == "tool_call" && shows(f, b, None) != Shows::No)
        })
        .expect("the call in the feed");
    assert!(at > 0, "the call is not the feed's first block");
    let moved = feed.blocks.remove(at);
    feed.blocks.insert(0, moved);
    assert!(!holds(&release, &current));
}

/// A set compared as a set is still one order on its side: two results swapped in the trace and not the session.
#[test]
fn a_set_shown_in_two_orders_on_one_side_fails() {
    let current = side("ag2/native/tool_use");
    let mut release = side("ag2/native@1.0.4/tool_use");
    let truth = release.2.clone();
    let trace = release
        .0
        .views
        .iter_mut()
        .find(|v| v.kind == ViewKind::Trace)
        .expect("the trace");
    let results: Vec<usize> = trace
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            truth
                .facts
                .iter()
                .any(|f| f.kind == "tool_result" && shows(f, b, None) != Shows::No)
        })
        .map(|(i, _)| i)
        .collect();
    assert!(results.len() >= 2);
    trace.blocks.swap(results[0], results[1]);
    assert!(!holds(&release, &current));
}

/// One response's unordered calls never share a run with another response's, nor with a batch of results.
#[test]
fn unordered_calls_group_by_their_response() {
    let fact = |id: &str| Fact {
        id: id.to_string(),
        kind: "tool_call".to_string(),
        role: "assistant".to_string(),
        conversation: String::new(),
        evidence: String::new(),
        value: json!({}),
        require: None,
        call: None,
        fixtures: None,
        seal: None,
        kept_from_model: false,
    };
    let proven = Proven {
        unordered: [("a", "call-1"), ("b", "call-1"), ("c", "call-2")]
            .into_iter()
            .map(|(f, c)| (f.to_string(), c.to_string()))
            .collect(),
        batches: vec![["r".to_string()].into_iter().collect()],
        ..Proven::default()
    };
    let (a, b, c, r) = (fact("a"), fact("b"), fact("c"), fact("r"));
    assert_eq!(group(&[&a], &proven), group(&[&b], &proven));
    assert_ne!(group(&[&a], &proven), group(&[&c], &proven));
    assert_ne!(group(&[&a], &proven), group(&[&r], &proven));
    assert_eq!(group(&[&a, &c], &proven), None);
}

/// Partial views that together state a cycle are not one order, however each pair reads alone.
#[test]
fn orders_stating_a_cycle_are_not_one_order() {
    let relation = |pairs: &[(&str, &str)]| {
        let mut after: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
        for (a, b) in pairs {
            after
                .entry(a.to_string())
                .or_default()
                .insert(b.to_string());
        }
        after
    };
    assert!(acyclic(&relation(&[("a", "b"), ("b", "c")])));
    assert!(!acyclic(&relation(&[("a", "b"), ("b", "c"), ("c", "a")])));
    assert!(!acyclic(&relation(&[("a", "b"), ("b", "a")])));
}

/// Two runs of one response's calls, a text between them, are two sets: the order gap does not let calls cross it.
#[test]
fn a_text_between_two_runs_of_calls_keeps_them_apart() {
    let mut current = side("ag2/native/tool_use");
    let call = current
        .2
        .calls
        .iter()
        .position(|c| {
            c.outputs
                .iter()
                .filter(|id| {
                    current
                        .2
                        .facts
                        .iter()
                        .any(|f| &f.id == *id && f.kind == "tool_call")
                })
                .count()
                >= 2
        })
        .expect("a response with parallel calls");
    // A response of runs A, text, B: split its calls by a text output.
    let calls: Vec<String> = current.2.calls[call].outputs.clone();
    current.2.calls[call].outputs = vec![calls[0].clone(), "a-text".to_string(), calls[1].clone()];
    current.2.facts.push(Fact {
        id: "a-text".to_string(),
        kind: "text".to_string(),
        role: "assistant".to_string(),
        conversation: String::new(),
        evidence: String::new(),
        value: json!({"text": "between"}),
        require: None,
        call: None,
        fixtures: None,
        seal: None,
        kept_from_model: false,
    });
    let mut release = current.2.clone();
    release.gaps.push(super::super::super::truth::Gap {
        fact: "response".to_string(),
        reason: "call_order_not_exported".to_string(),
        detail: String::new(),
        subject: Some(current.2.calls[call].id.clone()),
        modes: Vec::new(),
        text: None,
        name: None,
    });
    let proven = proven(&release, &current.2);
    let (a, b) = (&proven.unordered[&calls[0]], &proven.unordered[&calls[1]]);
    assert_ne!(a, b, "the two runs are two sets");
}

/// A set's member carries its set: a block of the same content from outside it cannot stand in for it.
#[test]
fn a_block_from_outside_a_set_cannot_stand_in_for_a_member() {
    let current = side("ag2/native/tool_use");
    let mut release = side("ag2/native@1.0.4/tool_use");
    let truth = release.2.clone();
    // The prompt, outside every set, moved into the run of parallel calls in every view but the feed.
    for view in release
        .0
        .views
        .iter_mut()
        .filter(|v| v.kind != ViewKind::Feed)
    {
        let prompt = view.blocks.iter().position(|b| {
            truth
                .facts
                .iter()
                .any(|f| f.kind == "user_text" && shows(f, b, None) != Shows::No)
        });
        let call = view.blocks.iter().rposition(|b| {
            truth
                .facts
                .iter()
                .any(|f| f.kind == "tool_call" && shows(f, b, None) != Shows::No)
        });
        if let (Some(p), Some(c)) = (prompt, call) {
            let moved = view.blocks.remove(p);
            view.blocks.insert(c, moved);
        }
    }
    assert!(!holds(&release, &current));
}

/// Every declared result encoding is what its release sent: each fixture it holds for carries, as a tool result the
/// rules read for the call it answers, the result's value in that encoding - for `python_str`, the value's Python
/// `repr`, as the result's text.
#[test]
fn every_result_encoding_is_in_its_release() {
    let fixtures: BTreeMap<String, Vec<PathBuf>> = crate::discover_fixtures().into_iter().collect();
    let mut proven = 0;
    let mut defects = Vec::new();
    for truth in super::super::super::truth::load_all().values() {
        for gap in truth.gaps.iter().filter(|g| g.reason == "result_encoding") {
            let fact = truth
                .facts
                .iter()
                .find(|f| Some(&f.id) == gap.subject.as_ref())
                .expect("the gap names a fact");
            let encoded = super::super::super::requests::python::repr(&fact.value["value"])
                .expect("a structured value has a repr");
            let call_id = fact.value["call_id"]
                .as_str()
                .expect("a result names its call");
            for fixture in truth.fixtures.iter().filter(|f| gap.holds_for(f)) {
                let Some(paths) = fixtures.get(fixture) else {
                    continue;
                };
                let (recon, _) = reconstruction(fixture, paths);
                let haystack = super::super::super::absence::haystack::Haystack::of_fixture(paths);
                if recorded_as(&recon, &haystack, call_id, &encoded) {
                    proven += 1;
                } else {
                    defects.push(format!("{fixture}: {} is not sent as {encoded}", fact.id));
                }
            }
        }
    }
    assert!(
        defects.is_empty(),
        "result encodings the payloads do not show:\n  {}",
        defects.join("\n  ")
    );
    assert!(proven > 0, "no declared result encoding was checked");
}

/// Whether a span the rules read a result answering `call_id` from carries `text` exactly as a bare text part in the
/// `content` of an object naming that call by one of its own members - the shape of an exported tool result - so
/// the witness is not the same characters on another span, in a resource attribute, or beside the text in a part.
///
/// A supporting proof, not the gate: the declaration rests on the release's cited encoder, and release invariance is
/// held by its own mutants. The residual hole: a payload holding, on such a span, the exact Python repr of the
/// tool's value inside an object that names the call and has a `content` list - somewhere other than the result's
/// own text - would satisfy it. Closing it means locating the occurrence the rules read the result from (the
/// block's carrier and position) in the payload, which the rubric cannot yet do (rule-language-program#170).
fn recorded_as(
    recon: &Recon,
    haystack: &super::super::super::absence::haystack::Haystack,
    call_id: &str,
    text: &str,
) -> bool {
    fn result(value: &serde_json::Value, call_id: &str, text: &str) -> bool {
        let bare = |part: &serde_json::Value| {
            part.as_object().is_some_and(|m| {
                m.get("text").and_then(serde_json::Value::as_str) == Some(text)
                    && m.keys().all(|k| k == "text" || k == "type")
            })
        };
        match value {
            serde_json::Value::Object(map) => {
                (map.values().any(|v| v.as_str() == Some(call_id))
                    && map
                        .get("content")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|parts| parts.iter().any(bare)))
                    || map.values().any(|v| result(v, call_id, text))
            }
            serde_json::Value::Array(items) => items.iter().any(|v| result(v, call_id, text)),
            _ => false,
        }
    }
    let spans: BTreeSet<(&str, &str)> = recon
        .views
        .iter()
        .flat_map(|v| &v.blocks)
        .filter(|b| b.is_tool_result() && b.result_call_id() == Some(call_id))
        .map(|b| (b.trace.as_str(), b.span.as_str()))
        .collect();
    haystack.carriers.iter().any(|carrier| {
        carrier
            .trace
            .as_deref()
            .zip(carrier.span.as_deref())
            .is_some_and(|key| spans.contains(&key))
            && carrier
                .nodes
                .iter()
                .any(|(_, node)| result(node, call_id, text))
    })
}

/// M6: a result the release encodes its own way is still held to its value.
#[test]
fn a_recoded_result_is_held_to_its_value() {
    let current = side("strands/native/tool_use");
    let release = side("strands/native@1.12.0/tool_use");
    assert!(
        holds(&release, &current),
        "the declared release is the same conversation"
    );
    let mut release = release;
    let truth = release.2.clone();
    for view in release.0.views.iter_mut() {
        for block in view.blocks.iter_mut() {
            let shows_result = truth
                .facts
                .iter()
                .any(|f| f.kind == "tool_result" && shows(f, block, None) != Shows::No);
            if shows_result && let Some(result) = block.content["content"].as_object_mut() {
                result.insert("city".into(), json!("Atlantis"));
                block.refresh();
            }
        }
    }
    assert!(!holds(&release, &current));
}

/// A release that declares no encoding is compared by digest: the same value written another way is a difference.
#[test]
fn an_undeclared_release_is_compared_by_digest() {
    let current = side("strands/native/tool_use");
    let mut release = side("strands/native@1.33.0/tool_use");
    let truth = release.2.clone();
    let mut recoded = 0;
    for view in release.0.views.iter_mut() {
        for block in view.blocks.iter_mut() {
            let text = block.content["content"].as_str().map(str::to_string);
            if let Some(text) = text
                && truth
                    .facts
                    .iter()
                    .any(|f| f.kind == "tool_result" && shows(f, block, None) != Shows::No)
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
            {
                block.content["content"] = value;
                block.refresh();
                recoded += 1;
            }
        }
    }
    assert!(recoded > 0, "the release shows its results as JSON text");
    assert!(!holds(&release, &current));
}

/// A recoded result is compared by value only in its content: any other member of the block - who ran the tool -
/// still has to agree.
#[test]
fn a_recoded_result_keeps_every_other_member_compared() {
    let current = side("strands/native/tool_use");
    let mut release = side("strands/native@1.12.0/tool_use");
    let truth = release.2.clone();
    for view in release.0.views.iter_mut() {
        for block in view.blocks.iter_mut() {
            if truth
                .facts
                .iter()
                .any(|f| f.kind == "tool_result" && shows(f, block, None) != Shows::No)
            {
                block.content["provider_executed"] = json!(true);
                block.refresh();
            }
        }
    }
    assert!(!holds(&release, &current));
}

/// The proof's witness is on a span the result was read from: the release's own payload holds it there, and no other
/// span's payload - or a part carrying more than the text - stands in for it.
#[test]
fn an_encoding_is_proven_only_where_the_result_was_read() {
    let label = "strands/native@1.12.0/streaming";
    let fixtures: BTreeMap<String, Vec<PathBuf>> = crate::discover_fixtures().into_iter().collect();
    let (recon, _) = reconstruction(label, &fixtures[label]);
    let haystack = super::super::super::absence::haystack::Haystack::of_fixture(&fixtures[label]);
    let call = recon
        .views
        .iter()
        .flat_map(|v| &v.blocks)
        .find_map(|b| b.is_tool_result().then(|| b.result_call_id()).flatten())
        .expect("a result")
        .to_string();
    let repr = "{'city': 'Rome', 'forecast': [{'day': 1, 'condition': 'sunny', 'high_c': 21}]}";
    assert!(recorded_as(&recon, &haystack, &call, repr));
    assert!(!recorded_as(&recon, &haystack, "another-call", repr));
    // The same characters only on spans no result was read from.
    let mut elsewhere =
        super::super::super::absence::haystack::Haystack::of_fixture(&fixtures[label]);
    let spans: BTreeSet<Option<String>> = recon
        .views
        .iter()
        .flat_map(|v| &v.blocks)
        .filter(|b| b.is_tool_result())
        .map(|b| Some(b.span.clone()))
        .collect();
    elsewhere.carriers.retain(|c| !spans.contains(&c.span));
    assert!(!recorded_as(&recon, &elsewhere, &call, repr));
}
