//! Whether a `sideseat.truth/3` document is consistent: on its own (ids, references, sequencing, gaps,
//! requirements) and with the repository (its source's digest, its fixtures).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::truth::*;

/// What is wrong with one document on its own: ids, references, sequencing, gaps, requirements.
pub(super) fn document_defects(key: &str, truth: &Truth) -> Vec<String> {
    let mut defects = Vec::new();
    let mut bad = |message: String| defects.push(format!("{key}: {message}"));

    if truth.format != FORMAT {
        bad(format!("format {:?}, expected {FORMAT}", truth.format));
    }
    if key != format!("{}/{}", truth.producer, truth.scenario) {
        bad(format!(
            "stored at {key} but names {}/{}",
            truth.producer, truth.scenario
        ));
    }
    if !["linear", "sessions", "multi_agent"].contains(&truth.topology.as_str()) {
        bad(format!("unknown topology {}", truth.topology));
    }
    if truth.generator.trim().is_empty() {
        bad("names no generator".to_string());
    }
    let bodies = match truth.source.kind.as_str() {
        "cassette" => "unrecorded",
        "fake-script" => "modelled",
        "program" => "program",
        other => {
            bad(format!("unknown source kind {other}"));
            ""
        }
    };
    if truth.request_bodies != bodies {
        bad(format!(
            "a {} source with request_bodies {}",
            truth.source.kind, truth.request_bodies
        ));
    }
    let successful = truth.calls.iter().filter(|c| c.succeeded()).count();
    if truth.model_calls != successful {
        bad(format!(
            "model_calls {} but {successful} successful calls",
            truth.model_calls
        ));
    }

    let mut ids = BTreeSet::new();
    for id in truth
        .calls
        .iter()
        .map(|c| &c.id)
        .chain(truth.facts.iter().map(|f| &f.id))
        .chain(truth.conversations.iter().map(|c| &c.id))
    {
        if !ids.insert(id.as_str()) {
            bad(format!("duplicate id {id}"));
        }
    }
    let facts: BTreeMap<&str, &Fact> = truth.facts.iter().map(|f| (f.id.as_str(), f)).collect();
    let calls: BTreeMap<&str, &Call> = truth.calls.iter().map(|c| (c.id.as_str(), c)).collect();
    let conversations: BTreeSet<&str> = truth.conversations.iter().map(|c| c.id.as_str()).collect();

    for (index, call) in truth.calls.iter().enumerate() {
        // A retry is the next attempt of the call before it, and only a failure is retried.
        let expected_attempt = match index.checked_sub(1).map(|i| &truth.calls[i]) {
            Some(previous) if !previous.succeeded() => previous.attempt + 1,
            _ => 1,
        };
        if call.attempt != expected_attempt {
            bad(format!(
                "{} is attempt {}, expected {expected_attempt}",
                call.id, call.attempt
            ));
        }
        if !conversations.contains(call.conversation.as_str()) {
            bad(format!("{} names unknown {}", call.id, call.conversation));
        }
        if !["success", "failed"].contains(&call.outcome.as_str()) {
            bad(format!("{} has outcome {}", call.id, call.outcome));
        }
        if !call.succeeded() && !call.outputs.is_empty() {
            bad(format!("failed {} lists outputs", call.id));
        }
        if call.api.is_empty() {
            bad(format!("{} names no api", call.id));
        }
        if !call.succeeded() && (call.error.is_none() || call.status.is_none_or(|s| s < 400)) {
            bad(format!("failed {} states no error and status", call.id));
        }
        for output in &call.outputs {
            match facts.get(output.as_str()) {
                None => bad(format!("{} outputs unknown {output}", call.id)),
                Some(fact) if fact.call.as_deref() != Some(call.id.as_str()) => bad(format!(
                    "{} outputs {output}, which names another call",
                    call.id
                )),
                Some(fact) if fact.conversation != call.conversation => bad(format!(
                    "{} outputs {output} of another conversation",
                    call.id
                )),
                Some(_) => {}
            }
        }
    }

    let mut sequenced: BTreeMap<&str, usize> = BTreeMap::new();
    for conversation in &truth.conversations {
        for id in &conversation.sequence {
            *sequenced.entry(id.as_str()).or_default() += 1;
            match facts.get(id.as_str()) {
                None => bad(format!("{} sequences unknown {id}", conversation.id)),
                Some(fact) if fact.conversation != conversation.id => bad(format!(
                    "{} sequences {id} of another conversation",
                    conversation.id
                )),
                Some(_) => {}
            }
        }
        for id in &conversation.final_answers {
            if !conversation.sequence.contains(id) {
                bad(format!("{} answers with unsequenced {id}", conversation.id));
            }
        }
    }
    for fact in &truth.facts {
        if sequenced.get(fact.id.as_str()) != Some(&1) {
            bad(format!("{} is not sequenced exactly once", fact.id));
        }
        if !conversations.contains(fact.conversation.as_str()) {
            bad(format!("{} names unknown {}", fact.id, fact.conversation));
        }
        let role = match fact.kind.as_str() {
            "system" => "system",
            "user_text" | "user_media" => "user",
            "text" | "reasoning" | "tool_call" => "assistant",
            "tool_result" => "tool",
            _ => "",
        };
        if fact.role != role {
            bad(format!(
                "{} is a {} with role {}",
                fact.id, fact.kind, fact.role
            ));
        }
        if !["wire", "wire-echo", "script", "program"].contains(&fact.evidence.as_str()) {
            bad(format!(
                "{} rests on unknown evidence {}",
                fact.id, fact.evidence
            ));
        }
        if let Some(call) = &fact.call {
            match calls.get(call.as_str()) {
                Some(c) if c.outputs.contains(&fact.id) => {}
                _ => bad(format!(
                    "{} names {call}, which does not output it",
                    fact.id
                )),
            }
        }
        match &fact.require {
            None => {}
            Some(require) => {
                let known_anchor = match require.anchor.as_str() {
                    "model_call" => fact.call.is_some(),
                    "conversation" => true,
                    _ => false,
                };
                if !known_anchor {
                    bad(format!(
                        "{} has unusable anchor {}",
                        fact.id, require.anchor
                    ));
                }
                if require.cardinality != "exactly_once" {
                    bad(format!(
                        "{} has cardinality {}",
                        fact.id, require.cardinality
                    ));
                }
                // Every view the anchor reaches, no fewer: a requirement that leaves a view out would
                // leave it unchecked.
                let expected: &[&str] = if require.anchor == "model_call" {
                    &["span", "trace", "session", "feed"]
                } else {
                    &["trace", "session", "feed"]
                };
                if require.views != expected {
                    bad(format!("{} has views {:?}", fact.id, require.views));
                }
                if !super::predicates::supports(&fact.kind, &require.matcher) {
                    bad(format!(
                        "{} is a {} matched by {}, which no predicate implements",
                        fact.id, fact.kind, require.matcher
                    ));
                }
            }
        }
    }
    let mut withdrawn: BTreeMap<&str, usize> = BTreeMap::new();
    for gap in &truth.gaps {
        if gap.detail.trim().is_empty() || gap.fact.trim().is_empty() {
            bad(format!(
                "gap {} names no category or explains nothing",
                gap.reason
            ));
        }
        // A closed vocabulary: the checks give each reason a meaning, and a gap about one fact or
        // call names it, so one gap cannot excuse many.
        let Some(effects) = gap_effects(&gap.reason) else {
            bad(format!("unknown gap reason {}", gap.reason));
            continue;
        };
        let names = |subject: &str| match effects.subject {
            GapSubject::Fact => facts.contains_key(subject),
            GapSubject::Call => calls.contains_key(subject),
            GapSubject::None => false,
        };
        match (&gap.subject, effects.subject) {
            (None, GapSubject::None) => {}
            (Some(subject), GapSubject::Fact | GapSubject::Call) if names(subject) => {}
            (Some(subject), GapSubject::Fact | GapSubject::Call) => {
                bad(format!("gap {} names unknown {subject}", gap.reason))
            }
            (None, _) => bad(format!("gap {} must name a subject", gap.reason)),
            (Some(_), GapSubject::None) => {
                bad(format!("gap {} must not name a subject", gap.reason))
            }
        }
        if gap.reason == "metadata_not_exported" && !METADATA_FIELDS.contains(&gap.fact.as_str()) {
            bad(format!(
                "gap metadata_not_exported names {}, not one of {METADATA_FIELDS:?}",
                gap.fact
            ));
        }
        let modes: BTreeSet<&str> = truth
            .fixtures
            .iter()
            .filter_map(|f| f.split('/').nth(1))
            .collect();
        for mode in &gap.modes {
            if !modes.contains(mode.as_str()) {
                bad(format!(
                    "gap {} names mode {mode}, which no fixture has",
                    gap.reason
                ));
            }
        }
        let Some(fact) = gap.subject.as_deref().and_then(|s| facts.get(s)) else {
            continue;
        };
        // A withdrawal that holds for every capture leaves the fact unasserted in the document. One that holds
        // for some modes only leaves it asserted, owed by the other captures; each capture of those modes is
        // checked without it (`for_fixture`).
        if effects.withdraws && gap.modes.is_empty() {
            *withdrawn.entry(fact.id.as_str()).or_default() += 1;
            if fact.require.is_some() {
                bad(format!(
                    "gap {} withdraws {}, which is still asserted",
                    gap.reason, fact.id
                ));
            }
        }
        if effects.withdraws && !gap.modes.is_empty() && fact.require.is_none() {
            bad(format!(
                "gap {} withdraws {} for some modes only, but the document leaves it unasserted for all",
                gap.reason, fact.id
            ));
        }
        if gap.reason == "kind_not_exported"
            && !(fact.kind == "reasoning" && fact.require.is_some() && !fact.text().is_empty())
        {
            bad(format!(
                "gap kind_not_exported on {} needs visible, asserted reasoning",
                fact.id
            ));
        }
        // An unexported id is a tool call's wire id; the fact keeps it, and checking moves it aside.
        if gap.reason == "id_not_exported"
            && !(fact.kind == "tool_call" && fact.value.get("id").is_some_and(Value::is_string))
        {
            bad(format!(
                "gap id_not_exported on {} needs a tool call with its wire id",
                fact.id
            ));
        }
        // An unexported signature is withheld reasoning's; checking then asserts it unsigned.
        if matches!(
            gap.reason.as_str(),
            "signature_not_exported" | "span_signature_not_exported"
        ) && !(fact.kind == "reasoning"
            && fact.require.as_ref().is_some_and(|r| r.matcher == "signed"))
        {
            bad(format!(
                "gap {} on {} needs asserted withheld reasoning",
                gap.reason, fact.id
            ));
        }
        // An output unexported on its own span is a response's part, owed there until the gap says not.
        if gap.reason == "output_not_exported"
            && !(fact.call.is_some()
                && fact
                    .require
                    .as_ref()
                    .is_some_and(|r| r.views.iter().any(|v| v == "span")))
        {
            bad(format!(
                "gap output_not_exported on {} needs a response part owed on its span",
                fact.id
            ));
        }
        // An absence is proven by searching for the fact's content, so the fact keeps all of it.
        if effects.needs_absence_proof && effects.withdraws && gap.fact != fact.kind {
            bad(format!(
                "gap {} on {} names category {}, not its kind {}",
                gap.reason, fact.id, gap.fact, fact.kind
            ));
        }
    }
    for fact in truth.facts.iter().filter(|f| f.require.is_none()) {
        match withdrawn.get(fact.id.as_str()) {
            Some(1) => {}
            None => bad(format!("{} is unasserted but no gap withdraws it", fact.id)),
            Some(n) => bad(format!("{} is withdrawn by {n} gaps", fact.id)),
        }
    }

    // The edges the order checks rely on must all be there, or those checks pass vacuously.
    let count = |kind: &str, from: &str| {
        truth
            .edges
            .iter()
            .filter(|e| e.kind == kind && e.from.as_deref() == Some(from))
            .count()
    };
    for fact in &truth.facts {
        let (kind, needed) = match fact.kind.as_str() {
            "user_text" => ("prompt_of", 1),
            "tool_result" => ("result_of", 1),
            _ => continue,
        };
        if count(kind, &fact.id) != needed {
            bad(format!("{} needs exactly one {kind} edge", fact.id));
        }
    }
    if truth.topology != "multi_agent" {
        for conversation in &truth.conversations {
            let ordered: Vec<&str> = truth
                .calls
                .iter()
                .filter(|c| c.conversation == conversation.id && c.succeeded())
                .map(|c| c.id.as_str())
                .collect();
            for pair in ordered.windows(2) {
                let linked = truth.edges.iter().any(|e| {
                    e.kind == "call_order"
                        && e.before.as_deref() == Some(pair[0])
                        && e.after.as_deref() == Some(pair[1])
                });
                if !linked {
                    bad(format!(
                        "{} and {} have no call_order edge",
                        pair[0], pair[1]
                    ));
                }
            }
        }
    }
    for edge in &truth.edges {
        let resolves = match edge.kind.as_str() {
            "prompt_of" => {
                edge.from
                    .as_deref()
                    .and_then(|f| facts.get(f))
                    .is_some_and(|f| f.kind == "user_text")
                    && edge.to.as_deref().is_some_and(|c| calls.contains_key(c))
            }
            "result_of" => {
                edge.from
                    .as_deref()
                    .and_then(|f| facts.get(f))
                    .is_some_and(|f| f.kind == "tool_result")
                    && edge
                        .to
                        .as_deref()
                        .and_then(|f| facts.get(f))
                        .is_some_and(|f| f.kind == "tool_call")
            }
            "call_order" => {
                edge.before
                    .as_deref()
                    .is_some_and(|c| calls.contains_key(c))
                    && edge.after.as_deref().is_some_and(|c| calls.contains_key(c))
            }
            _ => false,
        };
        if !resolves {
            bad(format!("edge {edge:?} does not resolve"));
        }
    }
    for (fixture, recorded) in &truth.requests {
        if !truth.fixtures.contains(fixture) {
            defects.push(format!(
                "{key}: requests for {fixture}, which is not one of its fixtures"
            ));
        }
        defects.extend(
            recorded
                .defects(fixture, truth)
                .into_iter()
                .map(|d| format!("{key}: {d}")),
        );
    }
    defects
}

/// What a document claims about the repository: its source's digest and its fixtures.
pub(super) fn repository_defects(key: &str, truth: &Truth) -> Vec<String> {
    use sha2::Digest as _;
    let mut defects = Vec::new();
    let source = repo_root().join(&truth.source.path);
    match std::fs::read(&source) {
        Ok(bytes) => {
            let digest = hex_digest(&sha2::Sha256::digest(&bytes));
            if digest != truth.source.sha256 {
                defects.push(format!(
                    "{key}: {} changed since the truth was derived; run `python -m harness truth {}`",
                    truth.source.path, truth.producer
                ));
            }
        }
        Err(e) => defects.push(format!("{key}: source {}: {e}", truth.source.path)),
    }
    for fixture in &truth.fixtures {
        if !super::super::fixture_root().join(fixture).is_dir() {
            defects.push(format!("{key}: fixture {fixture} does not exist"));
        }
    }
    defects
}

/// A withdrawal some releases only make leaves the fact owed by every other capture: the document keeps it
/// asserted, and only a capture of those releases is checked without it.
#[test]
fn a_withdrawal_for_some_releases_is_the_fixtures_alone() {
    let truths = load_all();
    let truth = &truths["ag2/tool_use"];
    let gap = truth
        .gaps
        .iter()
        .find(|g| g.reason == "not_exported" && !g.modes.is_empty())
        .expect("a not_exported gap scoped to releases");
    let subject = gap.subject.clone().expect("a fact");
    let asserted = |fixture: &str| {
        truth
            .for_fixture(fixture)
            .facts
            .iter()
            .find(|f| f.id == subject)
            .is_some_and(|f| f.require.is_some())
    };
    assert!(
        !asserted("ag2/native@1.0.2/tool_use"),
        "withdrawn where the release leaves it out"
    );
    assert!(
        asserted("ag2/native/tool_use"),
        "still owed where the release exports it"
    );
    assert!(document_defects("ag2/tool_use", truth).is_empty());
    // Unasserted in the document itself, the fact would be withdrawn from every capture: refused.
    let mut everywhere = truth.clone();
    for fact in everywhere.facts.iter_mut().filter(|f| f.id == subject) {
        fact.require = None;
    }
    assert!(
        document_defects("ag2/tool_use", &everywhere)
            .iter()
            .any(|d| d.contains("for some modes only")),
        "a release-scoped withdrawal of an unasserted fact is refused"
    );
}
